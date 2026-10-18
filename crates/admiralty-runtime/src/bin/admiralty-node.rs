const HELP: &str = "Usage: admiralty-node [--help|-h]\n\nNode enrollment is disabled until authenticated transport is qualified.\nThis entrypoint does not enroll nodes, launch providers or change a project.";

fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    if let Some(flag) = args.next()
        && (flag == "--help" || flag == "-h")
        && args.next().is_none()
    {
        println!("{HELP}");
        return std::process::ExitCode::SUCCESS;
    }
    eprintln!(
        "admiralty-node: enrollment is not enabled until authenticated transport is qualified"
    );
    std::process::ExitCode::from(78)
}
