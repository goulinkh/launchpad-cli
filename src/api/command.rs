use clap::ArgMatches;
use serde_json::{Value, json};

use crate::cli;
use crate::error::Error;
use crate::result::Result;

use super::contract::Contract;
use super::execute::call;

pub async fn run(arguments: &ArgMatches, dry_run: bool, yes: bool) -> Result<Value> {
    let (action, arguments) = arguments
        .subcommand()
        .ok_or_else(|| Error::invalid("API action is required"))?;
    let contract = Contract::embedded()?;
    match action {
        "schema" => match arguments.get_one::<String>("component") {
            Some(component) => Ok(contract.component(component)?.clone()),
            None => Ok(contract.document),
        },
        "operations" => {
            let filter = arguments.get_one::<String>("filter");
            let compact = arguments.get_flag("compact");
            let operations: Vec<_> = contract
                .operations
                .iter()
                .filter(|operation| {
                    filter.is_none_or(|filter| {
                        operation.operation_id.contains(filter) || operation.path.contains(filter)
                    })
                })
                .map(|operation| {
                    if compact {
                        Ok(operation.summary())
                    } else {
                        serde_json::to_value(operation).map_err(super::contract::contract_error)
                    }
                })
                .collect::<Result<_>>()?;
            Ok(json!({
                "operations": operations,
                "route_coverage": contract.document["x-launchpad-route-coverage"],
            }))
        }
        "describe" => {
            contract.describe(contract.operation(required_argument(arguments, "operation")?)?)
        }
        "decode" => {
            let component = required_argument(arguments, "component")?;
            let value = cli::read_input(required_argument(arguments, "input")?)?;
            contract.validate_component(component, &value)?;
            Ok(value)
        }
        "call" => {
            let operation = contract.operation(required_argument(arguments, "operation")?)?;
            call(
                &contract,
                operation,
                cli::input_object(arguments)?,
                dry_run,
                yes,
            )
            .await
        }
        _ => Err(Error::invalid("unknown API action")),
    }
}

fn required_argument<'arguments>(
    arguments: &'arguments ArgMatches,
    name: &str,
) -> Result<&'arguments str> {
    arguments
        .get_one::<String>(name)
        .map(String::as_str)
        .ok_or_else(|| Error::invalid(format!("{name} is required")))
}
