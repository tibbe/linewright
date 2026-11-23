//! The tools: one for each action, built from the action, with the
//! arguments that its screen takes and what it returns
//! (`docs/specs/access.md`, The MCP server, Tools and Arguments).

mod authorizations;
mod benchmarks;
mod people;

use rmcp::ErrorData;
use rmcp::handler::server::common::{schema_for_input, schema_for_output};
use rmcp::model::{CallToolRequestParams, JsonObject, Tool as Listed, ToolAnnotations};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::access::people::ManagerName;
use crate::access::{Caller, LevelMarker, Permit};
use crate::actions::Action;
use crate::api::Sources;

/// What a tool acts on.
pub(super) struct Context<'a> {
    pub(super) sources: &'a Sources,
    /// The `manager` setting.
    pub(super) manager: Option<&'a ManagerName>,
}

/// A tool, as its arguments: the action it gives, what it returns, and how
/// it runs.
trait Tool: DeserializeOwned + JsonSchema + 'static {
    const ACTION: Action;
    /// The access level of [`Tool::ACTION`].
    type Level: LevelMarker;
    type Output: Serialize + JsonSchema + 'static;

    /// What it would change, as the log of a refusal names it: nothing for
    /// a tool that only shows something.
    fn changing(&self) -> String {
        String::new()
    }

    /// Runs it with `permit`, giving what it returns, or else the message
    /// that the UI shows for the refusal or the failure.
    async fn run(
        self,
        permit: &Permit<Self::Level>,
        context: &Context<'_>,
    ) -> Result<Self::Output, String>;
}

/// What the log of a refusal names as changing for arguments that do not
/// read.
const UNREAD: &str = "what arguments that do not read name";

/// `T` as `tools/list` lists it: its action's tool name and the action as
/// its title, marked read-only where it only shows something, with its
/// arguments and what it returns.
fn listed<T: Tool>() -> Listed {
    let action = T::ACTION;
    let input = schema_for_input::<T>()
        .unwrap_or_else(|e| panic!("the arguments of {} are an object: {e}", action.tool_name()));
    let mut annotations = ToolAnnotations::new();
    if action.read_only() {
        annotations = annotations.read_only(true);
    }
    Listed::new_with_raw(action.tool_name(), None, input)
        .with_title(action.title())
        .with_raw_output_schema(schema_for_output::<T::Output>())
        .with_annotations(annotations)
}

/// Runs `T` as `caller`, with `arguments`: what it returns as JSON, or else
/// the message of the refusal, which for an access level that is too low
/// comes before any about the arguments.
async fn run<T: Tool>(
    arguments: Option<JsonObject>,
    caller: &Caller,
    context: &Context<'_>,
) -> Result<serde_json::Value, String> {
    let read =
        serde_json::from_value::<T>(serde_json::Value::Object(arguments.unwrap_or_default()));
    let changing = read
        .as_ref()
        .map_or_else(|_| UNREAD.to_string(), Tool::changing);
    let permit = T::ACTION
        .permit::<T::Level>(caller, &changing)
        .map_err(|needs| needs.to_string())?;
    let tool = read.map_err(|e| format!("The arguments do not read: {e}."))?;
    let output = tool.run(&permit, context).await?;
    Ok(serde_json::to_value(output).expect("what a tool returns serializes"))
}

/// The registry of the tools: what lists them, and what runs one by name.
macro_rules! registry {
    ($($tool:ty),* $(,)?) => {
        /// Every tool, with its action, in the order of the spec's table
        /// of actions.
        fn every() -> Vec<(Action, Listed)> {
            vec![$((<$tool as Tool>::ACTION, listed::<$tool>())),*]
        }

        /// Runs the tool `name` as `caller`, with `arguments`, `None` where
        /// there is no such tool.
        async fn run_named(
            name: &str,
            arguments: Option<JsonObject>,
            caller: &Caller,
            context: &Context<'_>,
        ) -> Option<Result<serde_json::Value, String>> {
            $(
                if name == <$tool as Tool>::ACTION.tool_name() {
                    return Some(run::<$tool>(arguments, caller, context).await);
                }
            )*
            None
        }
    };
}

registry!(
    people::ListPeople,
    authorizations::ListAuthorizations,
    authorizations::EndAuthorization,
    benchmarks::AddTask,
    benchmarks::CreateBenchmark,
    benchmarks::EditBenchmark,
    benchmarks::EditTask,
    benchmarks::RemoveTask,
    benchmarks::RunBenchmark,
    benchmarks::EditConfiguration,
    benchmarks::CancelBenchmarkRun,
    people::EditPerson,
    people::DisablePerson,
    people::EnablePerson,
);

/// The tools that `caller`'s role allows (`docs/specs/access.md`, The MCP
/// server, Listed tools).
pub(super) fn allowed(caller: &Caller) -> Vec<Listed> {
    every()
        .into_iter()
        .filter(|(action, _)| caller.can(action.level()))
        .map(|(_, listed)| listed)
        .collect()
}

/// The tool named `name`, whatever role it needs, if there is one.
pub(super) fn named(name: &str) -> Option<Listed> {
    every()
        .into_iter()
        .map(|(_, listed)| listed)
        .find(|listed| listed.name == name)
}

/// Calls the tool `request` names as `caller`: what it returns as JSON, or
/// else the message of its refusal, or the protocol's error where there is
/// no such tool.
pub(super) async fn call(
    context: &Context<'_>,
    caller: &Caller,
    request: CallToolRequestParams,
) -> Result<Result<serde_json::Value, String>, ErrorData> {
    run_named(&request.name, request.arguments, caller, context)
        .await
        .ok_or_else(|| ErrorData::invalid_params(format!("Unknown tool: {}", request.name), None))
}

/// The ID `given`, as `parse` reads it, or else the refusal of an ID that
/// names nothing: that `missing` does not exist.
fn id<T>(
    given: i64,
    parse: impl FnOnce(&str) -> Result<T, String>,
    missing: impl FnOnce(String) -> crate::actions::Missing,
) -> Result<T, String> {
    let given = given.to_string();
    parse(&given).map_err(|_| missing(given).to_string())
}

/// The name `given` as the argument `argument`, or else the refusal of a
/// required argument that holds no name.
fn name(argument: &str, given: &str) -> Result<crate::name::Name, String> {
    crate::name::Name::parse(given).map_err(|_| refused(argument, crate::actions::REQUIRED))
}

/// The refusal of the argument `argument` for `reason`.
fn refused(argument: &str, reason: &str) -> String {
    format!("{argument}: {reason}")
}
