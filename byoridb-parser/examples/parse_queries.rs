// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! nGQL parser example

use byoridb_parser::parse;

fn main() {
    println!("ByoriDB - nGQL Parser Example");
    println!("======================================\n");

    let queries = vec![
        "SHOW SPACES",
        "SHOW TAGS",
        "SHOW EDGES",
        "USE my_graph",
        "CREATE SPACE my_space",
        "CREATE SPACE IF NOT EXISTS my_space",
        "DROP SPACE IF NOT EXISTS old_space",
        "CREATE TAG player(name string, age int64)",
        "CREATE TAG IF NOT EXISTS player(name string, age int64, score float)",
        "CREATE EDGE follows(weight double)",
        "CREATE USER alice WITH PASSWORD 'secret'",
    ];

    for query in queries {
        print!("Query: {:50} => ", query);
        match parse(query) {
            Ok(stmt) => println!("✓ Parsed: {:?}", stmt),
            Err(e) => println!("✗ Error: {}", e),
        }
    }

    println!("\n✅ All examples completed!");
}
