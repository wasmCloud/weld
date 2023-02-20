

/// Generate the smithy file as the specified module name, inline.
/// if 'url' parameter missing and 'path' missing ,assumes url: "http...."
/// if just one file name, 
///     auto-imports wasmcloud-core and model
///     uses namespace from that file
///     
///    file has only one enty
/// assumes default wasmcloud-core.smithy, wasmcloud-model.smithy
/// and assumes base path of first parameter is '..... cdn ... wasmcloud/interfaecs"


mod httpserver { 
    smithy_bindgen_guest!({
       url: "https://cdn/github.com/wasmcloud/interfaces",
       files: [ "core/wasmcloud-core.smithy", "core/wasmcloud-model.smithy", "httpserver/httpserver.rs" ],
    }, "org.wasmcloud.interface.httpserver");
}
use httpserver::{HttpRequest, HttpResponse};

/// reduced form, makes some assumptions
///   - core and model are always included
///   - url is ....
///   - a single string param is the same as 'files: [ "rel-path.smithy" ]' appended to the 'url' base
/// future: with a single file, it should be possible to use the namespace of that file, but it would tak ea little more work
mod httpserver { 
    smithy_bindgen_guest!("httpserver/httpserver.rs", "org.wasmcloud.interace.httpserver");
}

/// local file, still loads core and model
/// a single 'path' with no 'files' means one file, and still pulls models in from default url
mod my_foo {
    smithy_bindgen_guest!({
        // can use 'path' (+ optional 'files') to specify file(s) at a local fs location 
        // can use 'url' (+ optional 'files') to specify file(s) at a remote url
        path: "../interface/foo.smithy",
    }, "org.example.foo");
}

// what if you want to override core/model (or exclude those?)
// just make sure the files are called wasmcloud-core.smithy and wasmcloud-model.smithy!!!
mod my_foo {
    smithy_bindgen_guest!([
        { url: "github.com/with/branch/and/rev", files: [ "wasmcloud-core.smithy", "wasmcloud-model.smithy" ] },
        { path: "../interface/foo.smithy" },
        ],
        "org.example.foo"
    );
}
