//! The tools: one for each action, built from the action, with the
//! arguments that its screen takes and what it returns
//! (`docs/specs/access.md`, The MCP server, Tools and Arguments).

mod authorizations;
mod benchmark_screens;
mod benchmarks;
mod people;
mod work;

use rmcp::ErrorData;
use rmcp::handler::server::common::{schema_for_input, schema_for_output};
use rmcp::model::{CallToolRequestParams, JsonObject, Tool as Listed, ToolAnnotations};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;

use linewright_core::access::people::ManagerName;
use linewright_core::access::{Caller, LevelMarker, NeedsLevel, Permit};
use linewright_core::actions::Action;
use linewright_core::api::Sources;

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
    let (read, permit) = permitted::<T>(arguments, caller);
    let permit = permit.map_err(|needs| needs.to_string())?;
    let tool = read?;
    let output = tool.run(&permit, context).await?;
    Ok(serde_json::to_value(output).expect("what a tool returns serializes"))
}

/// The refusal of `T` with `arguments` for an access level that `caller`'s
/// role does not reach, logged as [`Action::permit`] logs it, `None` where
/// it reaches it.
fn needs<T: Tool>(arguments: Option<JsonObject>, caller: &Caller) -> Option<NeedsLevel> {
    if caller.can(T::ACTION.level()) {
        return None;
    }
    permitted::<T>(arguments, caller).1.err()
}

/// `arguments` as [`read`] reads them as `T`, and `caller`'s permit of `T`,
/// or else the refusal, logged as [`Action::permit`] logs it.
#[expect(clippy::type_complexity, reason = "two results, each its own")]
fn permitted<T: Tool>(
    arguments: Option<JsonObject>,
    caller: &Caller,
) -> (Result<T, String>, Result<Permit<T::Level>, NeedsLevel>) {
    let read = read::<T>(serde_json::Value::Object(arguments.unwrap_or_default()));
    let changing = read
        .as_ref()
        .map_or_else(|_| UNREAD.to_string(), Tool::changing);
    let permit = T::ACTION.permit::<T::Level>(caller, &changing);
    (read, permit)
}

/// `arguments` as `T`, or else the refusal of a wrong argument: the first
/// that `T`'s input schema does not allow (`docs/specs/access.md`, The MCP
/// server, A wrong argument).
fn read<T: Tool>(arguments: serde_json::Value) -> Result<T, String> {
    let schema = schema_for_input::<T>().expect("the arguments of a tool are an object");
    let schema = serde_json::Value::Object(schema.as_ref().clone());
    let validator = jsonschema::validator_for(&schema).expect("a tool's input schema compiles");
    if let Some(wrong) = validator
        .iter_errors(&arguments)
        .find_map(|e| wrong_argument(&e))
    {
        return Err(format!("The argument {wrong} is not valid."));
    }
    serde_json::from_value(arguments).map_err(|e| format!("The arguments do not read: {e}."))
}

/// The argument that `error` is about: the first of its instance path;
/// or, for an error about the arguments as a whole, the missing one for
/// `required` and the unexpected one for `additionalProperties`, and else
/// `None`.
fn wrong_argument(error: &jsonschema::ValidationError<'_>) -> Option<String> {
    use jsonschema::error::ValidationErrorKind;
    use jsonschema::paths::LocationSegment;
    if let Some(LocationSegment::Property(name)) = error.instance_path().iter().next() {
        return Some(name.into_owned());
    }
    if let ValidationErrorKind::Required { property } = error.kind() {
        return property.as_str().map(str::to_string);
    }
    if let ValidationErrorKind::AdditionalProperties { unexpected } = error.kind() {
        return unexpected.first().cloned();
    }
    None
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

        /// The refusal of the tool `name` with `arguments` for an access
        /// level that `caller`'s role does not reach, logged as
        /// [`Action::permit`] logs it, `None` where it reaches it or where
        /// there is no such tool.
        pub(super) fn needs_level(
            name: &str,
            arguments: Option<JsonObject>,
            caller: &Caller,
        ) -> Option<NeedsLevel> {
            $(
                if name == <$tool as Tool>::ACTION.tool_name() {
                    return needs::<$tool>(arguments, caller);
                }
            )*
            None
        }
    };
}

registry!(
    work::GetBoard,
    work::GetItem,
    work::ListRuns,
    work::GetRun,
    work::ListAutomations,
    benchmark_screens::ListBenchmarks,
    benchmark_screens::GetBenchmark,
    benchmark_screens::GetTask,
    benchmark_screens::GetBenchmarkRun,
    benchmark_screens::GetTrial,
    work::GetServer,
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
    missing: impl FnOnce(String) -> linewright_core::actions::Missing,
) -> Result<T, String> {
    let given = given.to_string();
    parse(&given).map_err(|_| missing(given).to_string())
}

/// The name `given` as the argument `argument`, or else the refusal of a
/// required argument that holds no name.
fn name(argument: &str, given: &str) -> Result<linewright_core::name::Name, String> {
    linewright_core::name::Name::parse(given)
        .map_err(|_| refused(argument, linewright_core::actions::REQUIRED))
}

/// The refusal of the argument `argument` for `reason`.
fn refused(argument: &str, reason: &str) -> String {
    format!("{argument}: {reason}")
}
