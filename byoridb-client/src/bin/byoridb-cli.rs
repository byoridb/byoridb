use byoridb_client::Client;
use clap::Parser;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, Color, Table};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

#[derive(Parser, Debug)]
// `name` is set explicitly: without it clap reports CARGO_PKG_NAME, so
// `byoridb-cli --version` announced itself as "byoridb-client".
#[command(name = "byoridb-cli", version, about, long_about = None)]
struct Args {
    /// Server address
    #[arg(short, long, default_value = "127.0.0.1:9669")]
    addr: String,

    /// Username — required (env: BYORIDB_USER).
    /// No default; pass `--user <name>` or set `BYORIDB_USER=<name>` before
    /// invoking. The previous `root` default was removed because it leaked
    /// into production deployments.
    #[arg(short, long, env = "BYORIDB_USER")]
    user: String,

    /// Password — required (env: BYORIDB_PASSWORD).
    /// Prefer the env var so the password does not appear in shell history.
    #[arg(short, long, env = "BYORIDB_PASSWORD")]
    password: String,

    /// Execute a single query and exit
    #[arg(short, long)]
    execute: Option<String>,
}

fn print_result(result: serde_json::Value) {
    // Handle empty results (DDL statements like CREATE/DROP/ALTER)
    if result.is_null() {
        println!("Executed successfully.");
        return;
    }

    if let Some(arr) = result.as_array() {
        if arr.is_empty() {
            println!("Executed successfully.");
            return;
        }
    }

    // Check if it's a dataset with headers and rows
    if let Some(obj) = result.as_object() {
        // Check for empty dataset
        if let Some(rows) = obj.get("rows") {
            if let Some(rows_arr) = rows.as_array() {
                if rows_arr.is_empty() {
                    // Check if there are column names (indicates a query with no results vs DDL)
                    if let Some(headers) = obj.get("column_names") {
                        if let Some(headers_arr) = headers.as_array() {
                            if headers_arr.is_empty() {
                                println!("Executed successfully.");
                                return;
                            }
                        }
                    } else {
                        println!("Executed successfully.");
                        return;
                    }
                }
            }
        }

        if let (Some(headers), Some(rows)) = (obj.get("column_names"), obj.get("rows")) {
            let mut table = Table::new();
            table
                .load_preset(UTF8_FULL)
                .set_content_arrangement(comfy_table::ContentArrangement::Dynamic);

            // Add headers
            if let Some(headers_arr) = headers.as_array() {
                let header_cells: Vec<Cell> = headers_arr
                    .iter()
                    .map(|h| Cell::new(h.as_str().unwrap_or("")).fg(Color::Green))
                    .collect();
                table.set_header(header_cells);
            }

            // Add rows
            if let Some(rows_arr) = rows.as_array() {
                if rows_arr.is_empty() {
                    println!("Empty set.");
                    return;
                }

                for row in rows_arr {
                    let values = if let Some(row_obj) = row.as_object() {
                        row_obj.get("values").and_then(|v| v.as_array())
                    } else {
                        row.as_array()
                    };

                    if let Some(vals) = values {
                        let row_cells: Vec<Cell> =
                            vals.iter().map(|v| Cell::new(format_value(v))).collect();
                        table.add_row(row_cells);
                    }
                }
            }

            println!("{}", table);
            return;
        }
    }

    // Fallback: just print pretty JSON
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_else(|_| "Invalid JSON".to_string())
    );
}

fn format_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_value).collect();
            format!("[{}]", items.join(", "))
        }
        serde_json::Value::Object(obj) => {
            // Handle tagged enum format from serde (e.g., {"String": "value"}, {"Int": 42})
            if obj.len() == 1 {
                if let Some((key, val)) = obj.iter().next() {
                    match key.as_str() {
                        "String" => return format_value(val),
                        "Int" => return format_value(val),
                        "Float" => return format_value(val),
                        "Bool" => return format_value(val),
                        "Null" => return "NULL".to_string(),
                        "Empty" => return "".to_string(),
                        "Vertex" => return format_vertex(val),
                        "Edge" => return format_edge(val),
                        "List" => return format_value(val),
                        "Map" => return format_map(val),
                        "Date" | "Time" | "DateTime" => return format_value(val),
                        _ => {}
                    }
                }
            }
            // Fallback for other objects
            format_object(obj)
        }
    }
}

fn format_vertex(v: &serde_json::Value) -> String {
    if let Some(obj) = v.as_object() {
        let vid = obj
            .get("vid")
            .map(format_value)
            .unwrap_or_else(|| "?".to_string());
        let tags = obj
            .get("tags")
            .and_then(|t| t.as_array())
            .map(|tags| {
                tags.iter()
                    .filter_map(|tag| {
                        let name = tag.get("name")?.as_str()?;
                        Some(format!(":{}", name))
                    })
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        format!("({}{})", vid, tags)
    } else {
        format_value(v)
    }
}

fn format_edge(v: &serde_json::Value) -> String {
    if let Some(obj) = v.as_object() {
        let src = obj
            .get("src")
            .map(format_value)
            .unwrap_or_else(|| "?".to_string());
        let dst = obj
            .get("dst")
            .map(format_value)
            .unwrap_or_else(|| "?".to_string());
        let name = obj.get("name").and_then(|n| n.as_str()).unwrap_or("?");
        format!("({})-[:{}]->({})", src, name, dst)
    } else {
        format_value(v)
    }
}

fn format_map(v: &serde_json::Value) -> String {
    if let Some(obj) = v.as_object() {
        let items: Vec<String> = obj
            .iter()
            .map(|(k, val)| format!("{}: {}", k, format_value(val)))
            .collect();
        format!("{{{}}}", items.join(", "))
    } else {
        format_value(v)
    }
}

fn format_object(obj: &serde_json::Map<String, serde_json::Value>) -> String {
    let items: Vec<String> = obj
        .iter()
        .map(|(k, v)| format!("{}: {}", k, format_value(v)))
        .collect();
    format!("{{{}}}", items.join(", "))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Connect to server
    let mut client =
        match Client::connect(args.addr.clone(), args.user.clone(), args.password.clone()).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to connect to {}: {}", args.addr, e);
                std::process::exit(1);
            }
        };

    println!("Connected to byoridb-server at {}", args.addr);

    if let Some(query) = args.execute {
        match client.execute_json(&query).await {
            Ok(result) => print_result(result),
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // REPL mode
    let mut rl = DefaultEditor::new()?;
    if rl.load_history("history.txt").is_err() {
        // No previous history
    }

    loop {
        let readline = rl.readline("byoridb> ");
        match readline {
            Ok(line) => {
                let line = line.trim();
                if line.eq_ignore_ascii_case("exit") || line.eq_ignore_ascii_case("quit") {
                    break;
                }
                if line.is_empty() {
                    continue;
                }

                rl.add_history_entry(line)?;

                // Execute query
                match client.execute_json(line).await {
                    Ok(res) => print_result(res),
                    Err(e) => eprintln!("Error: {}", e),
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("CTRL-C");
                break;
            }
            Err(ReadlineError::Eof) => {
                println!("CTRL-D");
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
            }
        }
    }

    // Try to close session gracefully
    let _ = client.close().await;

    rl.save_history("history.txt")?;
    Ok(())
}
