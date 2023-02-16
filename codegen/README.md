# Code generation from Smithy IDL models

Code generation is implemented in Rust, and output languages currently include Rust and Html (for documentation). We are expecting to add more languages in the future.

The code generator can be invoked:
- as a library
- from build.rs file: to build project sources according to a plan in [`codegen.toml`](https://wassmcloud.dev/interfaces/codegen-toml/)
- from the [`wash` cli](https://github.com/wasmcloud/wash)

Documentation on how the code generator works, how to use it with wasmCloud, and how to extend it, can be found in the [developer documentation](https://wasmcloud.dev/interfaces/).

## Environment Variables

The following environment variables can be used to modify the operation of `codegen`:

| Variable         | Default                                              | Example              | Description                                            |
|------------------|------------------------------------------------------|----------------------|--------------------------------------------------------|
| `WELD_CACHE_DIR` | [`$XDG_CACHE_DIR`/`$HOME/.cache`][directories-crate] | `/path/to/cache/dir` | Override cache directory used by `weld`                |
| `SMITHY_CACHE`   | N/A                                                  | `NO_EXPIRE`          | Alter behavior (force enable) of the smithy file cache |

[directories-crate]: https://crates.io/crates/directories
