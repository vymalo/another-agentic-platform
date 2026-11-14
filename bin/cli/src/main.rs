//! `aapctl`: see the library for what it does. This only gives it the process's streams and maps its answer to the exit code.

use std::io;
use std::process::ExitCode;

fn main() -> ExitCode {
    let code = aap_cli::run(
        std::env::args_os(),
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    );
    ExitCode::from(code)
}
