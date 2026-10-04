//! Tool calls for engines with no tool-call API (RFC-unified-chat 6e).
//!
//! Foundation Models and the agent CLIs answer text, not `tool_calls`. So
//! that every engine can run the chat loop, a round on one of them becomes a
//! plain chat: the tools are listed in the system prompt, the model answers
//! with one JSON object naming a tool, and that object is parsed and checked
//! against what was offered. A reply that names no tool is the model being
//! done. One malformed attempt gets one corrective retry; a second is read
//! as done, never guessed at.

use serde_json::Value;

use super::{ChatTurn, ToolCall};

/// How a reply reads.
#[derive(Debug, PartialEq)]
pub(crate) enum Parsed {
    /// A call to one of the offered tools.
    Call { name: String, arguments: Value },
    /// No tool: the model is done.
    Done,
    /// It tried to call something and got it wrong: a tool that wasn't
    /// offered, or JSON that doesn't parse.
    Malformed(String),
}

/// The protocol, appended to the loop's own system prompt.
fn protocol(tools: &[Value]) -> String {
    let mut listing = String::new();
    for t in tools {
        let f = &t["function"];
        let name = f["name"].as_str().unwrap_or_default();
        let params: Vec<String> = f["parameters"]["properties"]
            .as_object()
            .map(|p| p.keys().cloned().collect())
            .unwrap_or_default();
        let required: Vec<&str> = f["parameters"]["required"]
            .as_array()
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let params = params
            .iter()
            .map(|p| {
                if required.contains(&p.as_str()) {
                    p.clone()
                } else {
                    format!("{p}?")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        listing.push_str(&format!(
            "- {name}({params}): {}\n",
            f["description"].as_str().unwrap_or_default()
        ));
    }
    format!(
        "TOOLS. You can call these, one at a time:\n{listing}\n\
         To call one, reply with ONLY a JSON object and nothing else:\n\
         {{\"tool\": \"<name>\", \"arguments\": {{...}}}}\n\
         Its result comes back in the next message. When you need no more tools, \
         reply with {{\"done\": true}}. Do not use any tools of your own; only these."
    )
}

/// The loop's tool conversation as plain chat turns. Assistant rows that
/// called a tool become the JSON object the protocol asks for; tool results
/// become user messages that say which tool answered.
pub(crate) fn to_turns(messages: &[Value], tools: &[Value]) -> Vec<ChatTurn> {
    let mut turns: Vec<ChatTurn> = Vec::new();
    let mut system_done = false;
    for m in messages {
        let role = m["role"].as_str().unwrap_or("user");
        let content = m["content"].as_str().unwrap_or_default();
        match role {
            "system" if !system_done => {
                turns.push(ChatTurn::system(format!(
                    "{content}\n\n{}",
                    protocol(tools)
                )));
                system_done = true;
            }
            "assistant" => {
                let calls = m["tool_calls"].as_array().cloned().unwrap_or_default();
                let text = match calls.first() {
                    Some(c) => serde_json::json!({
                        "tool": c["function"]["name"],
                        "arguments": c["function"]["arguments"],
                    })
                    .to_string(),
                    None => content.to_string(),
                };
                turns.push(ChatTurn {
                    role: "assistant".into(),
                    content: text,
                });
            }
            "tool" => {
                let name = m["tool_name"].as_str().unwrap_or("the tool");
                turns.push(ChatTurn::user(format!("Result of {name}:\n{content}")));
            }
            _ => turns.push(ChatTurn {
                role: role.to_string(),
                content: content.to_string(),
            }),
        }
    }
    if !system_done {
        turns.insert(0, ChatTurn::system(protocol(tools)));
    }
    turns
}

/// The first balanced `{...}` in a reply. Models wrap the object in a code
/// fence or a sentence despite being asked not to; the object is what counts.
fn first_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, ch) in text[start..].char_indices() {
        if in_string {
            match ch {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Read a reply against the tools that were offered.
pub(crate) fn parse(text: &str, tools: &[Value]) -> Parsed {
    let Some(obj) = first_object(text) else {
        return Parsed::Done;
    };
    let Ok(v) = serde_json::from_str::<Value>(obj) else {
        // Braces but no valid JSON: an attempt, if it mentions a tool.
        return if obj.contains("\"tool\"") {
            Parsed::Malformed("that wasn't valid JSON".into())
        } else {
            Parsed::Done
        };
    };
    if v["done"].as_bool() == Some(true) {
        return Parsed::Done;
    }
    let Some(name) = v["tool"].as_str() else {
        return Parsed::Done;
    };
    let offered = tools
        .iter()
        .any(|t| t["function"]["name"].as_str() == Some(name));
    if !offered {
        return Parsed::Malformed(format!("there is no tool named {name}"));
    }
    let arguments = match &v["arguments"] {
        Value::Object(_) => v["arguments"].clone(),
        // Some models send the arguments as a JSON string.
        Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({})),
        _ => serde_json::json!({}),
    };
    Parsed::Call {
        name: name.to_string(),
        arguments,
    }
}

/// The corrective message for one retry after a malformed reply.
pub(crate) fn retry_turn(why: &str) -> ChatTurn {
    ChatTurn::user(format!(
        "That didn't work: {why}. Reply with ONLY one JSON object: \
         {{\"tool\": \"<name from the list>\", \"arguments\": {{...}}}}, or {{\"done\": true}}."
    ))
}

/// A parsed call in the loop's own shape.
pub(crate) fn to_call(name: String, arguments: Value, round: usize) -> ToolCall {
    ToolCall {
        id: format!("text-{round}"),
        name,
        arguments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<Value> {
        vec![json!({
            "type": "function",
            "function": {
                "name": "create_notebook",
                "description": "Create a notebook.",
                "parameters": {
                    "type": "object",
                    "properties": { "title": {}, "urls": {} },
                    "required": ["title"]
                }
            }
        })]
    }

    #[test]
    fn a_fenced_call_parses_and_unknown_tools_are_malformed() {
        let t = tools();
        let reply = "Sure.\n```json\n{\"tool\": \"create_notebook\", \"arguments\": {\"title\": \"GPUs {2026}\"}}\n```";
        assert_eq!(
            parse(reply, &t),
            Parsed::Call {
                name: "create_notebook".into(),
                arguments: json!({ "title": "GPUs {2026}" }),
            }
        );
        assert!(matches!(
            parse("{\"tool\": \"delete_everything\", \"arguments\": {}}", &t),
            Parsed::Malformed(_)
        ));
        assert!(matches!(
            parse("{\"tool\": \"create_notebook\", \"arguments\": {", &t),
            Parsed::Malformed(_) | Parsed::Done
        ));
        assert_eq!(parse("{\"done\": true}", &t), Parsed::Done);
        assert_eq!(parse("The answer is in your notes.", &t), Parsed::Done);
    }

    #[test]
    fn string_arguments_are_read_as_json() {
        let reply = r#"{"tool": "create_notebook", "arguments": "{\"title\": \"X\"}"}"#;
        assert_eq!(
            parse(reply, &tools()),
            Parsed::Call {
                name: "create_notebook".into(),
                arguments: json!({ "title": "X" }),
            }
        );
    }

    #[test]
    fn the_tool_conversation_becomes_plain_turns() {
        let messages = vec![
            json!({ "role": "system", "content": "You work in a library." }),
            json!({ "role": "user", "content": "make a GPU notebook" }),
            json!({ "role": "assistant", "content": "", "tool_calls": [
                { "id": "1", "type": "function",
                  "function": { "name": "create_notebook", "arguments": { "title": "GPUs" } } }
            ]}),
            json!({ "role": "tool", "tool_call_id": "1", "tool_name": "create_notebook",
                    "content": "Created the notebook **GPUs**." }),
        ];
        let turns = to_turns(&messages, &tools());
        assert_eq!(turns[0].role, "system");
        assert!(turns[0]
            .content
            .contains("- create_notebook(title, urls?): Create a notebook."));
        assert_eq!(turns[2].role, "assistant");
        assert!(turns[2].content.contains("\"tool\":\"create_notebook\""));
        assert_eq!(turns[3].role, "user");
        assert!(turns[3].content.starts_with("Result of create_notebook:"));
    }
}
