use std::path::PathBuf;

use gumdrop::Options;
use hashgoblin::{Error, HashType, audit, create, verbose_init};

#[derive(Options)]
struct Args {
    #[options(help = "print help message")]
    help: bool,
    #[options(help = "maxim number of threads the program may sapwn, DEFAULT: 5")]
    max_threads: Option<u8>,
    #[options(help = "generate hashes recursevely")]
    recursive: bool,
    #[options(
        help = "unless this option is present, empty directories will be ignored by default"
    )]
    empty_dirs: bool,
    #[options(help = "prints detailed information during this program execution")]
    verbose: bool,
    #[options(command)]
    command: Option<Command>,
}

#[derive(Options)]
struct CreateOpts {
    #[options(
        free,
        help = "source file or directory. If it is a directory, recursive option must also be enabled"
    )]
    source: Vec<String>,
    #[options(
        help = "hash algorithm, suported: sha256, tiger, whirlpool, sha1, md5, default: sha256",
        short = "H"
    )]
    hash: Option<HashType>,
    #[options(help = "path to the output file, default: ./hashes.txt")]
    output: Option<PathBuf>,
}

#[derive(Options)]
struct AuditOpts {
    #[options(
        free,
        help = "source file or directory. If it is a directory, recursive option must also be enabled"
    )]
    source: Vec<String>,
    #[options(help = "exit early on the first audit mismatch", short = "E")]
    early: bool,
    #[options(help = "path to the hashes file, default ./hashes.txt", short = "f")]
    hashes_file: Option<PathBuf>,
}

#[derive(Options)]
struct HelpOpts {
    #[options(free)]
    free: Vec<String>,
}

#[derive(Options)]
enum Command {
    #[options(help = "show help for a command")]
    Help(HelpOpts),
    #[options(help = "create a new hashes file that can be audited later with the audit command")]
    Create(CreateOpts),
    #[options(help = "audit a source directory or files against a hashes file")]
    Audit(AuditOpts),
}

fn main() -> Result<(), Error> {
    let args = Args::parse_args_default_or_exit();
    verbose_init(args.verbose);
    match args.command {
        Some(Command::Create(opts)) => create(
            &opts.source,
            args.recursive,
            args.max_threads.unwrap_or(5),
            opts.hash.unwrap_or(HashType::SHA256),
            opts.output,
            args.empty_dirs,
        ),
        Some(Command::Audit(opts)) => audit(
            &opts.source,
            args.recursive,
            args.max_threads.unwrap_or(5),
            opts.hashes_file,
            opts.early,
            args.empty_dirs,
        ),
        None => {
            println!("You must specify a command, use --help [COMMAND] for more information\n");
            println!("{}\n", args.self_usage());
            println!("Available Commands:\n{}", args.self_command_list().unwrap());
            Ok(())
        }
        _ => Ok(()),
    }
}
