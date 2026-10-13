use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    slonbase::cli::run(slonbase::Extensions::default()).await
}
