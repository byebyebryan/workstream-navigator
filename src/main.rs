use std::process::ExitCode;

const ABOUT: &str =
    "A native-workflow terminal navigator for persistent coding workstreams across hosts.";

fn main() -> ExitCode {
    run(std::env::args().skip(1))
}

fn run(args: impl IntoIterator<Item = String>) -> ExitCode {
    let mut args = args.into_iter();

    match (args.next().as_deref(), args.next()) {
        (None | Some("-h" | "--help"), None) => {
            print_help();
            ExitCode::SUCCESS
        }
        (Some("-V" | "--version"), None) => {
            println!("wsnav {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("error: no workstream commands are implemented yet");
            eprintln!("try 'wsnav --help'");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!("Workstream Navigator");
    println!();
    println!("{ABOUT}");
    println!();
    println!("Usage: wsnav [OPTIONS]");
    println!();
    println!("Options:");
    println!("  -h, --help       Print help");
    println!("  -V, --version    Print version");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_invocation_is_successful() {
        assert_eq!(run([]), ExitCode::SUCCESS);
    }

    #[test]
    fn unknown_argument_fails() {
        assert_eq!(run(["unknown".to_owned()]), ExitCode::FAILURE);
    }
}
