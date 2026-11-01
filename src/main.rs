use std::process::ExitCode;

// musl's allocator (the release binaries are static musl builds) serializes threads on every
// allocation, which halved throughput; mimalloc is fast on musl and glibc alike. Set here,
// in the binary, so programs using slonbase as a library choose their own.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> ExitCode {
    slonbase::cli::run(slonbase::Extensions::default()).await
}
