fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [] => receive::diagnostics::run(),
        [flag, name] if flag == "--domain" => receive::diagnostics::domain(name),
        _ => Err(anyhow::anyhow!(
            "Usage: receive-diagnose [--domain example.com]"
        )),
    };
    if let Err(error) = result {
        eprintln!("Diagnostics: {error:#}");
        std::process::exit(1);
    }
}
