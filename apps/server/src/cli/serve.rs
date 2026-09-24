use std::io;
use std::process::ExitCode;

use crate::app::lifecycle::{self, ShutdownSignals};
use crate::config::EnvironmentSource;
use crate::infra::telemetry::write_startup_failure;

pub fn serve() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return bootstrap_failed("the async runtime could not be started", &error),
    };
    runtime.block_on(async {
        let signals = match ShutdownSignals::install() {
            Ok(signals) => signals,
            Err(error) => {
                return bootstrap_failed(
                    "the shutdown signal handlers could not be installed",
                    &error,
                )
            }
        };
        lifecycle::run(&EnvironmentSource::from_process(), signals).await
    })
}

fn bootstrap_failed(context: &str, error: &io::Error) -> ExitCode {
    let _ = write_startup_failure(
        &mut io::stderr().lock(),
        &format_args!("{context}: {error}"),
    );
    ExitCode::FAILURE
}

#[cfg(feature = "openapi-export")]
pub fn export_openapi() -> ExitCode {
    use std::io::Write;

    let written = crate::app::openapi::export_document()
        .map_err(|error| error.to_string())
        .and_then(|document| {
            let mut stdout = io::stdout().lock();
            stdout
                .write_all(&document)
                .and_then(|()| stdout.flush())
                .map_err(|error| error.to_string())
        });
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "palmr: {error}");
            ExitCode::FAILURE
        }
    }
}
