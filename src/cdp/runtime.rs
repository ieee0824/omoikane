//! `Runtime` domain: expression evaluation, remote objects, and the runtime
//! helpers installed into each page.

use super::*;

impl CdpSession {
    pub(super) fn runtime_evaluate(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let expression = require_string(params, "expression")?;
        let return_by_value = params
            .get("returnByValue")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let result = self.evaluate_expression(&expression, return_by_value)?;
        self.drive_navigation_requests()?;
        Ok(result)
    }

    pub(super) fn runtime_call_function_on(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
        let function_declaration = require_string(params, "functionDeclaration")?;
        let return_by_value = params
            .get("returnByValue")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let object_id = params.get("objectId").and_then(Value::as_str);
        if let Some(object_id) = object_id {
            self.validate_remote_object(object_id)?;
        }
        let this_value = match params.get("objectId").and_then(Value::as_str) {
            Some(object_id) => {
                self.runtime
                    .remote_object(object_id)
                    .ok_or_else(|| JsonRpcError {
                        code: -32000,
                        message: format!("Cannot find remote object: {object_id}"),
                    })?
            }
            None => JsValue::undefined(),
        };

        let mut arguments = Vec::new();
        for argument in params
            .get("arguments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(object_id) = argument.get("objectId").and_then(Value::as_str) {
                self.validate_remote_object(object_id)?;
                arguments.push(self.runtime.remote_object(object_id).ok_or_else(|| {
                    JsonRpcError {
                        code: -32000,
                        message: format!("Cannot find remote object: {object_id}"),
                    }
                })?);
            } else if let Some(value) = argument.get("value") {
                // The value came from serde_json, so its textual form is a
                // data literal rather than page-provided source. Parentheses
                // keep object literals from being parsed as statement blocks.
                let source = format!("({value})");
                arguments.push(self.runtime.eval(&source).map_err(js_error)?);
            } else {
                return Err(invalid_params(
                    "Each Runtime.callFunctionOn argument must include value or objectId"
                        .to_string(),
                ));
            }
        }
        let value = self
            .runtime
            .call_function_with_arguments(&function_declaration, this_value, arguments)
            .map_err(js_error)?;
        self.runtime.run_until_idle().map_err(js_error)?;
        let result = self.serialize_evaluation_value(value, return_by_value)?;
        self.drive_navigation_requests()?;
        Ok(result)
    }

    pub(super) fn runtime_release_object(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let object_id = require_string(params, "objectId")?;
        self.validate_remote_object(&object_id)?;
        self.runtime.release_remote_object(&object_id);
        self.remote_object_generations.remove(&object_id);
        Ok(json!({}))
    }

    fn validate_remote_object(&self, object_id: &str) -> Result<(), JsonRpcError> {
        if self
            .remote_object_generations
            .get(object_id)
            .is_some_and(|generation| *generation == self.document_generation)
        {
            return Ok(());
        }
        Err(JsonRpcError {
            code: -32000,
            message: format!("Could not find object with given id: {object_id}"),
        })
    }

    fn evaluate_expression(
        &mut self,
        expression: &str,
        return_by_value: bool,
    ) -> Result<Value, JsonRpcError> {
        let value = self.runtime.eval(expression).map_err(js_error)?;
        // Runtime.evaluate is itself a user-agent task. Complete its
        // microtask checkpoint and make any host tasks (such as navigation)
        // ready before the protocol method commits them.
        self.runtime.run_until_idle().map_err(js_error)?;
        self.serialize_evaluation_value(value, return_by_value)
    }

    pub(super) fn serialize_evaluation_value(
        &mut self,
        value: JsValue,
        return_by_value: bool,
    ) -> Result<Value, JsonRpcError> {
        let object_id = if return_by_value {
            None
        } else {
            let object_id = format!("remote-{}", self.next_object_id);
            self.next_object_id += 1;
            Some(object_id)
        };
        let retained_value = object_id.as_ref().map(|_| value.clone());
        let serialization_function = if return_by_value {
            "value => JSON.stringify({ result: __cdpSerializeValue(value) })".to_string()
        } else {
            let object_id = object_id
                .as_deref()
                .expect("non-value serialization always has a remote object id");
            format!(
                "value => JSON.stringify({{ result: __cdpRemoteObject(value, {object_id:?}) }})"
            )
        };
        let raw = self
            .runtime
            .call_function_with_value(&serialization_function, value)
            .map_err(js_error)?;
        // Match the synchronous Runtime.evaluate task boundary: serializer
        // getters/toJSON may enqueue microtasks that must settle before the
        // protocol response and any resulting navigation are committed.
        self.runtime.run_until_idle().map_err(js_error)?;
        let payload = raw
            .as_string()
            .ok_or(JsonRpcError {
                code: -32000,
                message: "Runtime evaluation did not return a string payload".to_string(),
            })?
            .to_std_string_escaped();
        let result: Value = serde_json::from_str(&payload).map_err(|error| JsonRpcError {
            code: -32000,
            message: error.to_string(),
        })?;
        if let Some(object_id) = object_id {
            if result
                .get("result")
                .and_then(|result| result.get("objectId"))
                .is_some()
            {
                self.runtime.retain_remote_object(
                    object_id.clone(),
                    retained_value.expect("remote object value was cloned above"),
                );
                self.remote_object_generations
                    .insert(object_id, self.document_generation);
            }
        }
        Ok(result)
    }

    pub(super) fn install_runtime_helpers(&mut self) -> Result<(), boa_engine::JsError> {
        Self::install_runtime_helpers_on(&mut self.runtime)
    }

    pub(super) fn install_runtime_helpers_on(
        runtime: &mut JsRuntime,
    ) -> Result<(), boa_engine::JsError> {
        runtime.eval(
            r#"
            globalThis.__cdpSerializeValue = function(value) {
              if (value === undefined) return { type: "undefined" };
              if (value === null) return { type: "object", subtype: "null", value: null };
              if (typeof value === "number") return { type: "number", value };
              if (typeof value === "string") return { type: "string", value };
              if (typeof value === "boolean") return { type: "boolean", value };
              if (typeof value === "function") return { type: "function", description: String(value) };
              return { type: "object", value: JSON.parse(JSON.stringify(value)) };
            };
            globalThis.__cdpRemoteObject = function(value, objectId) {
              if (value === null) return { type: "object", subtype: "null", value: null };
              return {
                type: typeof value === "object" ? "object" : typeof value,
                objectId,
                description: Object.prototype.toString.call(value),
              };
            };
            "#,
        )?;
        runtime.run_jobs()
    }
}
