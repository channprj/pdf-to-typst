use std::process::ExitCode;

fn main() -> ExitCode {
    match pdf_to_typst::parse_args(std::env::args_os()) {
        Ok(pdf_to_typst::ParseResult::Help) => {
            print!("{}", pdf_to_typst::help_text());
            ExitCode::SUCCESS
        }
        Ok(pdf_to_typst::ParseResult::Version) => {
            println!("{}", pdf_to_typst::version_text());
            ExitCode::SUCCESS
        }
        Ok(pdf_to_typst::ParseResult::Run(options)) => match pdf_to_typst::run(&options) {
            Ok(success) => {
                if !success.warnings.is_empty() {
                    for warning in success.warnings {
                        eprintln!("warning: {}", warning.message());
                    }
                }

                println!("{}", success.main_typ.display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(error.exit_code as u8)
            }
        },
        Err(error) => {
            eprintln!("{error}");
            if error.print_help {
                eprintln!();
                eprintln!("{}", pdf_to_typst::help_text());
            }
            ExitCode::from(error.exit_code as u8)
        }
    }
}
