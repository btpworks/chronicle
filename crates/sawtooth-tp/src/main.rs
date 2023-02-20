mod abstract_tp;
mod tp;
use clap::{builder::PossibleValuesParser, Arg, Command, ValueHint};
use sawtooth_sdk::processor::TransactionProcessor;

use ::chronicle_telemetry::ConsoleLogging;
use chronicle_telemetry::telemetry;
use tokio::runtime::Handle;
use tp::ChronicleTransactionHandler;
use tracing::info;
use url::Url;

#[tokio::main]
async fn main() {
    let matches = Command::new("chronicle-sawtooth-tp")
        .version("1.0")
        .author("Blockchain technology partners")
        .about("Write and query provenance data to distributed ledgers")
        .arg(
            Arg::new("connect")
                .short('C')
                .long("connect")
                .value_hint(ValueHint::Url)
                .help("Sets sawtooth validator address")
                .takes_value(true),
        )
        .arg(
            Arg::new("completions")
                .long("completions")
                .value_name("completions")
                .value_parser(PossibleValuesParser::new(["bash", "zsh", "fish"]))
                .help("Generate shell completions and exit"),
        )
        .arg(
            Arg::new("instrument")
                .long("instrument")
                .value_name("instrument")
                .takes_value(true)
                .value_hint(ValueHint::Url)
                .help("Instrument using RUST_LOG environment"),
        )
        .arg(
            Arg::new("console-logging")
                .long("console-logging")
                .value_name("console-logging")
                .takes_value(true)
                .default_value("pretty")
                .help("Log to console using RUST_LOG environment"),
        )
        .get_matches();

    telemetry(
        matches
            .get_one::<String>("instrument")
            .and_then(|s| Url::parse(s).ok()),
        match matches.get_one::<String>("console-logging") {
            Some(level) => match level.as_str() {
                "pretty" => ConsoleLogging::Pretty,
                "json" => ConsoleLogging::Json,
                _ => ConsoleLogging::Off,
            },
            _ => ConsoleLogging::Off,
        },
    );

    let (policy, entrypoint) = ("default_allow.wasm", "default_allow.allow");

    Handle::current().spawn_blocking(move || {
        info!(
            "Starting Chronicle Transaction Processor on {:?}",
            matches.get_one::<String>("connect")
        );
        let handler = match ChronicleTransactionHandler::new(policy, entrypoint) {
            Ok(handler) => handler,
            Err(e) => panic!("Error initializing TransactionHandler: {e}"),
        };
        let mut processor = TransactionProcessor::new({
            if let Some(connect) = matches.get_one::<String>("connect") {
                connect
            } else {
                "tcp://127.0.0.1:4004"
            }
        });

        processor.add_handler(&handler);
        processor.start();
    });
}
