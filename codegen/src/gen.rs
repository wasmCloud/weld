//! This module contains [Generator](#Generator) - config-driven code generation,
//! and [CodeGen](#CodeGen), the trait for language-specific code-driven code generation.
//!
//!
use std::{
    borrow::Borrow,
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

use atelier_core::model::{
    shapes::{AppliedTraits, HasTraits as _},
    HasIdentity as _, Identifier, Model,
};

use crate::{
    codegen_asm::AsmCodeGen,
    codegen_go::GoCodeGen,
    codegen_py::PythonCodeGen,
    codegen_rust::RustCodeGen,
    config::{CodegenConfig, LanguageConfig, OutputFile, OutputLanguage},
    docgen::DocGen,
    error::{Error, Result},
    model::{get_trait, serialization_trait, CommentKind, NumberedMember},
    render::Renderer,
    wasmbus_model::{RenameItem, Serialization},
    writer::Writer,
    Bytes, JsonValue, ParamMap, TomlValue,
};

/// Common templates compiled-in
pub const COMMON_TEMPLATES: &[(&str, &str)] = &[];

/// A Generator is a data-driven wrapper around code generator implementations,
/// There are two main modes of generation:
/// handlebars template-driven, and code-driven. For the latter, you implement
/// a trait CodeGen, which gets callbacks for various parts of the code generation.
/// One parameter to CodeGen is the output file name, which can be used if code is
/// to be generated across several files. (for example, if you want
/// one file to define interfaces and a separate file to define implementation classe).
/// Handlebars-driven code generation may be more well-suited for files such as Makefiles
/// and project config files that don't need a huge amount of customization,
/// but they can also be used to generate 100% of an interface. You decide!
#[derive(Debug, Default)]
pub struct Generator {}

impl<'model> Generator {
    /// Perform code generation on model, iterating through each configured OutputFile.
    /// Each file to be generated is either based on a handlebars
    /// template, which is generated with the Renderer class,
    /// or is generated by an implemented of the CodeGen implementation.
    /// Function parameters:
    /// - model - the smithy model
    /// - config - CodeConfig (usually loaded from a codegen.toml file)
    ///     - N.B. all relative paths (template dirs and the output_dir parameter) are
    ///            adjusted to be relative to config.base_dir.
    /// - templates - list of additional templates to register with the handlebars engine
    ///          (The templates parameter of config is ignored. to use a list of file
    ///          paths, call templates_from_dir() to load them and generate this parameter)
    /// - output_dir - top-level folder containing all output files
    /// - create - whether the user intends to create a new project (true) or just update (false).
    ///   The generator does not check for existence of an output file before overwriting. Usually the
    ///   `create` flag is set to true for the first pass when project files are created, after which
    ///   time the project file can be manually edited, and only the main codegen output will be updated.
    ///   The default (create is false/the flag is unspecified)) changes the fewest files
    pub fn gen(
        &self,
        model: Option<&'model Model>,
        config: CodegenConfig,
        templates: Vec<(String, String)>,
        output_dir: &Path,
        defines: Vec<(String, TomlValue)>,
    ) -> Result<()> {
        let mut json_model = match model {
            Some(model) => atelier_json::model_to_json(model),
            None => JsonValue::default(),
        };
        let output_dir = if output_dir.is_absolute() {
            output_dir.to_path_buf()
        } else {
            config.base_dir.join(output_dir)
        };
        // create one renderer so we only need to parse templates once
        let mut renderer = Renderer::default();

        for (name, template) in COMMON_TEMPLATES.iter() {
            renderer.add_template((name, template))?;
        }
        std::fs::create_dir_all(&output_dir).map_err(|e| {
            Error::Io(format!(
                "creating directory {}: {}",
                &output_dir.display(),
                e
            ))
        })?;

        for (language, mut lc) in config.languages.into_iter() {
            if !config.output_languages.is_empty() && !config.output_languages.contains(&language) {
                // if user specified list of languages, only generate code for those languages
                continue;
            }
            // add templates from <lang>.templates
            if let Some(template_dir) = &lc.templates {
                let template_dir = if template_dir.is_absolute() {
                    template_dir.clone()
                } else {
                    config.base_dir.join(template_dir)
                };
                for (name, tmpl) in templates_from_dir(&template_dir)? {
                    renderer.add_template((&name, &tmpl))?;
                }
            }
            // add templates from cli
            for (name, template) in templates.iter() {
                renderer.add_template((name, template))?;
            }
            // if language output_dir is relative, append it, otherwise use it
            let output_dir = if lc.output_dir.is_absolute() {
                std::fs::create_dir_all(&lc.output_dir).map_err(|e| {
                    Error::Io(format!(
                        "creating directory {}: {}",
                        &lc.output_dir.display(),
                        e
                    ))
                })?;
                lc.output_dir.clone()
            } else {
                output_dir.join(&lc.output_dir)
            };
            // add command-line overrides
            for (k, v) in defines.iter() {
                lc.parameters.insert(k.to_string(), v.clone());
            }
            let base_params: BTreeMap<String, JsonValue> = to_json(&lc.parameters)?;

            let mut cgen = gen_for_language(&language, model);

            // initialize generator
            cgen.init(model, &lc, &output_dir, &mut renderer)?;

            // A common param dictionary is shared (read-only) by the renderer and the code generator,
            // Parameters include the following:
            //   - "model" - the entire model, generated by parsing one or more smithy files,
            //               and converted to json-ast format
            //   - "_file" - the output file (path is relative to the output directory
            //   - LanguageConfig.parameters , applicable to all files for this output language
            //   - OutputFile.parameters - file-specific parameters
            //   The latter two are added in that order, so that a per-file parameter can
            //   override per-language settings.

            // The parameter dict is cleared after each iteration to avoid one file's override
            // from leaking into the next file in the iteration. There are two "optimizations"
            // in the loop below.
            // - The handlebars renderer only parses templates once, so it is shared across output files,
            // - The smithy model is parsed and validated once. After using it for one file, we pull it
            //   out of the params map, save it aside, and then re-insert it the next time.

            // list of files that were created or updated on this run
            let mut updated_files = Vec::new();

            for file_config in lc.files.iter() {
                // for conditional files (with the `if_defined` property), check that we have the right conditions
                if let Some(TomlValue::String(key)) = file_config.params.get("if_defined") {
                    match lc.parameters.get(key) {
                        None | Some(TomlValue::Boolean(false)) => {
                            // not defined, do not generate
                            continue;
                        }
                        Some(_) => {}
                    }
                }
                let mut params = base_params.clone();
                params.insert("model".to_string(), json_model);

                let file_params: BTreeMap<String, JsonValue> = to_json(&file_config.params)?;
                params.extend(file_params.into_iter());
                params.insert(
                    "_file".to_string(),
                    JsonValue::String(file_config.path.to_string_lossy().to_string()),
                );

                let out_path = output_dir.join(&file_config.path);
                let parent = out_path.parent().unwrap();
                std::fs::create_dir_all(parent).map_err(|e| {
                    Error::Io(format!("creating directory {}: {}", parent.display(), e))
                })?;

                // generate output using either hbs or CodeGen
                if let Some(hbs) = &file_config.hbs {
                    let mut out = std::fs::File::create(&out_path).map_err(|e| {
                        Error::Io(format!("creating file {}: {}", &out_path.display(), e))
                    })?;
                    renderer.render(hbs, &params, &mut out)?;
                    out.flush().map_err(|e| {
                        crate::Error::Io(format!(
                            "saving output file {}:{}",
                            &out_path.display(),
                            e
                        ))
                    })?;
                } else if let Some(model) = model {
                    let bytes = cgen.generate_file(model, file_config, &params)?;
                    std::fs::write(&out_path, &bytes).map_err(|e| {
                        Error::Io(format!("writing output file {}: {}", out_path.display(), e))
                    })?;
                };
                updated_files.push(out_path);
                // retrieve json_model for the next iteration
                json_model = params.remove("model").unwrap();
            }
            cgen.format(updated_files, &lc.parameters)?;
        }

        Ok(())
    }
}

fn gen_for_language<'model>(
    language: &OutputLanguage,
    model: Option<&'model Model>,
) -> Box<dyn CodeGen + 'model> {
    match language {
        OutputLanguage::Rust => Box::new(RustCodeGen::new(model)),
        OutputLanguage::AssemblyScript => Box::new(AsmCodeGen::new(model)),
        OutputLanguage::Python => Box::new(PythonCodeGen::new(model)),
        OutputLanguage::TinyGo => Box::new(GoCodeGen::new(model, true)),
        OutputLanguage::Go => Box::new(GoCodeGen::new(model, false)),
        OutputLanguage::Html => Box::new(DocGen::default()),
        OutputLanguage::Poly => Box::new(PolyGen::default()),
        _ => {
            crate::error::print_warning(&format!("Target language {} not implemented", language));
            Box::new(NoCodeGen::default())
        }
    }
}

/// A Codegen is used to generate source for a Smithy Model
/// The generator will invoke these functions (in order)
/// - init()
/// - write_source_file_header()
/// - declare_types()
/// - write_services()
/// - finalize()
///
pub(crate) trait CodeGen {
    /// Initialize code generator and renderer for language output.j
    /// This hook is called before any code is generated and can be used to initialize code generator
    /// and/or perform additional processing before output files are created.
    #[allow(unused_variables)]
    fn init(
        &mut self,
        model: Option<&Model>,
        lc: &LanguageConfig,
        output_dir: &Path,
        renderer: &mut Renderer,
    ) -> std::result::Result<(), Error> {
        Ok(())
    }

    /// This entrypoint drives output-file-specific code generation.
    /// This default implementation invokes `init_file`, `write_source_file_header`, `declare_types`, `write_services`, and `finalize`.
    /// The return value is Bytes containing the data that should be written to the output file.
    fn generate_file(
        &mut self,
        model: &Model,
        file_config: &OutputFile,
        params: &ParamMap,
    ) -> Result<Bytes> {
        let mut w: Writer = Writer::default();

        self.init_file(&mut w, model, file_config, params)?;
        self.write_source_file_header(&mut w, model, params)?;
        self.declare_types(&mut w, model, params)?;
        self.write_services(&mut w, model, params)?;
        self.finalize(&mut w)
    }

    /// Perform any initialization required prior to code generation for a file
    /// `model` may be used to check model metadata
    /// `id` is a tag from codegen.toml that indicates which source file is to be written
    /// `namespace` is the namespace in the model to generate
    #[allow(unused_variables)]
    fn init_file(
        &mut self,
        w: &mut Writer,
        model: &Model,
        file_config: &OutputFile,
        params: &ParamMap,
    ) -> Result<()> {
        Ok(())
    }

    /// generate the source file header
    #[allow(unused_variables)]
    fn write_source_file_header(
        &mut self,
        w: &mut Writer,
        model: &Model,
        params: &ParamMap,
    ) -> Result<()> {
        Ok(())
    }

    /// Write declarations for simple types, maps, and structures
    #[allow(unused_variables)]
    fn declare_types(&mut self, w: &mut Writer, model: &Model, params: &ParamMap) -> Result<()> {
        Ok(())
    }

    /// Write service declarations and implementation stubs
    #[allow(unused_variables)]
    fn write_services(&mut self, w: &mut Writer, model: &Model, params: &ParamMap) -> Result<()> {
        Ok(())
    }

    /// Complete generation and return the output bytes
    fn finalize(&mut self, w: &mut Writer) -> Result<Bytes> {
        Ok(w.take().freeze())
    }

    /// Write documentation for item
    #[allow(unused_variables)]
    fn write_documentation(&mut self, w: &mut Writer, _id: &Identifier, text: &str) {
        for line in text.split('\n') {
            // remove whitespace from end of line
            let line = line.trim_end_matches(|c| c == '\r' || c == ' ' || c == '\t');
            self.write_comment(w, CommentKind::Documentation, line);
        }
    }

    /// Writes single-line comment beginning with '// '
    /// Can be overridden if more specific kinds are needed
    #[allow(unused_variables)]
    fn write_comment(&mut self, w: &mut Writer, kind: CommentKind, line: &str) {
        w.write(b"// ");
        w.write(line);
        w.write(b"\n");
    }

    fn write_ident(&self, w: &mut Writer, id: &Identifier) {
        w.write(&self.to_type_name_case(&id.to_string()));
    }

    /// append suffix to type name, for example "Game", "Context" -> "GameContext"
    fn write_ident_with_suffix(
        &mut self,
        w: &mut Writer,
        id: &Identifier,
        suffix: &str,
    ) -> Result<()> {
        self.write_ident(w, id);
        w.write(suffix); // assume it's already PascalCase
        Ok(())
    }

    // Writes info the the current output writer
    //fn write(&mut self, bytes: impl ToBytes);

    // Returns the current buffer, zeroing out self
    //fn take(&mut self) -> BytesMut;

    /// Returns current output language
    fn output_language(&self) -> OutputLanguage;

    fn has_rename_trait(&self, traits: &AppliedTraits) -> Option<String> {
        if let Ok(Some(items)) = get_trait::<Vec<RenameItem>>(traits, crate::model::rename_trait())
        {
            let lang = self.output_language().to_string();
            return items.iter().find(|i| i.lang == lang).map(|i| i.name.clone());
        }
        None
    }

    /// returns file extension of source files for this language
    fn get_file_extension(&self) -> &'static str {
        self.output_language().extension()
    }

    /// Convert method name to its target-language-idiomatic case style
    fn to_method_name_case(&self, name: &str) -> String;

    /// Convert method name to its target-language-idiomatic case style
    /// implementors should override to_method_name_case
    fn to_method_name(&self, method_id: &Identifier, method_traits: &AppliedTraits) -> String {
        if let Some(name) = self.has_rename_trait(method_traits) {
            name
        } else {
            self.to_method_name_case(&method_id.to_string())
        }
    }

    /// Convert field name to its target-language-idiomatic case style
    fn to_field_name_case(&self, name: &str) -> String;

    /// Convert field name to its target-language-idiomatic case style
    /// implementors should override to_field_name_case
    fn to_field_name(
        &self,
        member_id: &Identifier,
        member_traits: &AppliedTraits,
    ) -> std::result::Result<String, Error> {
        if let Some(name) = self.has_rename_trait(member_traits) {
            Ok(name)
        } else {
            Ok(self.to_field_name_case(&member_id.to_string()))
        }
    }

    /// Convert type name to its target-language-idiomatic case style
    fn to_type_name_case(&self, s: &str) -> String;

    fn get_field_name_and_ser_name(&self, field: &NumberedMember) -> Result<(String, String)> {
        let field_name = self.to_field_name(field.id(), field.traits())?;
        let ser_name = if let Some(Serialization { name: Some(ser_name) }) =
            get_trait(field.traits(), serialization_trait())?
        {
            ser_name
        } else {
            field.id().to_string()
        };
        Ok((field_name, ser_name))
    }

    /// The operation name used in dispatch, from method
    /// The default implementation is provided and should not be overridden
    fn op_dispatch_name(&self, id: &Identifier) -> String {
        crate::strings::to_pascal_case(&id.to_string())
    }

    /// The full operation name with service prefix
    /// The default implementation is provided and should not be overridden
    fn full_dispatch_name(&self, service_id: &Identifier, method_id: &Identifier) -> String {
        format!(
            "{}.{}",
            &self.to_type_name_case(&service_id.to_string()),
            &self.op_dispatch_name(method_id)
        )
    }

    fn source_formatter(&self) -> Result<Box<dyn SourceFormatter>> {
        Ok(Box::new(crate::format::NullFormatter::default()))
    }

    /// After code generation has completed for all files, this method is called once per output language
    /// to allow code formatters to run. The `files` parameter contains a list of all files written or updated.
    fn format(
        &mut self,
        files: Vec<PathBuf>,
        lc_params: &BTreeMap<String, TomlValue>,
    ) -> Result<()> {
        // if we just created an interface project, don't run rustfmt yet
        // because we haven't generated the other rust file yet, so rustfmt will fail.
        if !lc_params.contains_key("create_interface") {
            // make a list of all output files with ".rs" extension so we can fix formatting with rustfmt
            // minor nit: we don't check the _config-only flag so there could be some false positives here, but rustfmt is safe to use anyway
            let formatter = self.source_formatter()?;

            let extension = self.output_language().extension();
            let sources = files
                .into_iter()
                .filter(|path| match path.extension() {
                    Some(s) => s.to_string_lossy().as_ref() == extension,
                    _ => false,
                })
                .collect::<Vec<PathBuf>>();

            if !sources.is_empty() {
                ensure_files_exist(&sources)?;

                let file_names: Vec<std::borrow::Cow<'_, str>> =
                    sources.iter().map(|p| p.to_string_lossy()).collect();
                let borrowed = file_names.iter().map(|s| s.borrow()).collect::<Vec<&str>>();
                formatter.run(&borrowed)?;
            }
        }
        Ok(())
    }
}

/// Formats source code
#[allow(unused_variables)]
pub trait SourceFormatter {
    /// run formatter on all files
    /// Default implementation does nothing
    fn run(&self, source_files: &[&str]) -> Result<()> {
        Ok(())
    }
}

/// confirm all files are present, otherwise return error
fn ensure_files_exist(source_files: &[std::path::PathBuf]) -> Result<()> {
    let missing = source_files
        .iter()
        .filter(|p| !p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<String>>();
    if !missing.is_empty() {
        return Err(Error::Formatter(format!(
            "missing source file(s) '{}'",
            missing.join(",")
        )));
    }
    Ok(())
}

#[derive(Debug, Default)]
struct PolyGen {}
impl CodeGen for PolyGen {
    fn output_language(&self) -> OutputLanguage {
        OutputLanguage::Poly
    }
    /// generate method name
    fn to_method_name_case(&self, name: &str) -> String {
        crate::strings::to_snake_case(name)
    }

    /// generate field name
    fn to_field_name_case(&self, name: &str) -> String {
        crate::strings::to_snake_case(name)
    }

    /// generate type name
    fn to_type_name_case(&self, name: &str) -> String {
        crate::strings::to_pascal_case(name)
    }
}

#[allow(dead_code)]
/// helper function for indenting code (used by python codegen)
pub fn spaces(indent_level: u8) -> &'static str {
    const SP: &str =
        "                                                                                         \
         \
                                                                                                  ";
    &SP[0..((indent_level * 4) as usize)]
}

// convert from TOML map to JSON map so it's usable by handlebars
//pub fn toml_to_json(map: &BTreeMap<String, TomlValue>) -> Result<ParamMap> {
//    let s = serde_json::to_string(map)?;
//    let value: ParamMap = serde_json::from_str(&s)?;
//    Ok(value)
//}

/// Converts a type to json
pub fn to_json<S: serde::Serialize, T: serde::de::DeserializeOwned>(val: S) -> Result<T> {
    let s = serde_json::to_string(&val)?;
    Ok(serde_json::from_str(&s)?)
}

/// Search a folder recursively for files ending with the provided extension
/// Filenames must be utf-8 characters
pub fn find_files(dir: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    if dir.is_dir() {
        let mut results = Vec::new();
        for entry in std::fs::read_dir(dir)
            .map_err(|e| Error::Io(format!("reading directory {}: {}", dir.display(), e)))?
        {
            let entry = entry.map_err(|e| crate::Error::Io(format!("scanning folder: {}", e)))?;
            let path = entry.path();
            if path.is_dir() {
                results.append(&mut find_files(&path, extension)?);
            } else {
                let ext = path
                    .extension()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if ext == extension {
                    results.push(path)
                }
            }
        }
        Ok(results)
    } else if dir.is_file()
        && &dir
            .extension()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
            == "smithy"
    {
        Ok(vec![dir.to_owned()])
    } else {
        Err(Error::Other(format!(
            "'{}' is not a valid folder or '.{}' file",
            dir.display(),
            extension
        )))
    }
}

/// Add all templates from the specified folder, using the base file name
/// as the template name. For example, "header.hbs" will be registered as "header"
pub fn templates_from_dir(start: &std::path::Path) -> Result<Vec<(String, String)>> {
    let mut templates = Vec::new();

    for path in crate::gen::find_files(start, "hbs")?.iter() {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if !stem.is_empty() {
            let template = std::fs::read_to_string(path)
                .map_err(|e| Error::Io(format!("reading template {}: {}", path.display(), e)))?;
            templates.push((stem, template));
        }
    }
    Ok(templates)
}

#[derive(Default)]
struct NoCodeGen {}
impl CodeGen for NoCodeGen {
    fn output_language(&self) -> OutputLanguage {
        OutputLanguage::Poly
    }

    fn get_file_extension(&self) -> &'static str {
        ""
    }

    fn to_method_name_case(&self, name: &str) -> String {
        crate::strings::to_snake_case(name)
    }

    fn to_field_name_case(&self, name: &str) -> String {
        crate::strings::to_snake_case(name)
    }

    fn to_type_name_case(&self, name: &str) -> String {
        crate::strings::to_pascal_case(name)
    }
}
