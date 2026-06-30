// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Parser tests

use super::*;
use crate::ast::*;

#[test]
fn test_parse_show_spaces() {
    let result = parse("SHOW SPACES");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Show(ShowStatement::Spaces) => {}
        _ => panic!("Expected ShowSpaces"),
    }
}

#[test]
fn test_parse_use_space() {
    let result = parse("USE my_space");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Use(stmt) => {
            assert_eq!(stmt.space, "my_space");
        }
        _ => panic!("Expected Use statement"),
    }
}

#[test]
fn test_parse_create_tag() {
    let result = parse("CREATE TAG player(name STRING, age INT64)");
    assert!(result.is_ok());
}

#[test]
fn test_parse_create_space() {
    let result = parse("CREATE SPACE IF NOT EXISTS my_space");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Create(CreateStatement::Space(stmt)) => {
            assert_eq!(stmt.name, "my_space");
            assert!(stmt.if_not_exists);
        }
        _ => panic!("Expected CreateSpace"),
    }
}

#[test]
fn test_parse_insert_vertex() {
    let result = parse("INSERT VERTEX player(name, age) VALUES 1:('Alice', 30)");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Insert(stmt) => {
            assert_eq!(stmt.insert_type, InsertType::Vertex);
            assert_eq!(stmt.vertices.len(), 1);
            assert_eq!(stmt.vertices[0].tags.len(), 1);
            assert_eq!(stmt.vertices[0].tags[0].name, "player");
        }
        _ => panic!("Expected Insert statement"),
    }
}

#[test]
fn test_parse_delete_vertex() {
    let result = parse("DELETE VERTEX 1, 2, 3");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Delete(stmt) => {
            assert_eq!(stmt.delete_type, DeleteType::Vertex);
            assert_eq!(stmt.vids.len(), 3);
        }
        _ => panic!("Expected Delete statement"),
    }
}

#[test]
fn test_parse_fetch() {
    let result = parse("FETCH PROP ON player 1, 2");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Fetch(stmt) => {
            assert_eq!(stmt.tags, vec!["player"]);
            assert_eq!(stmt.vids.len(), 2);
        }
        _ => panic!("Expected Fetch statement"),
    }
}

#[test]
fn test_parse_go() {
    let result = parse("GO FROM 1 OVER follow");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Go(stmt) => {
            assert_eq!(stmt.over_edges, vec!["follow"]);
            assert_eq!(stmt.from_clause.vids.len(), 1);
        }
        _ => panic!("Expected Go statement"),
    }
}

#[test]
fn test_parse_go_with_steps() {
    let result = parse("GO 2 STEPS FROM 1 OVER follow");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Go(stmt) => {
            assert_eq!(stmt.to_clause.steps, StepClause::Exactly(2));
        }
        _ => panic!("Expected Go statement"),
    }
}

#[test]
fn test_parse_go_from_var_qualified() {
    let result = parse("GO FROM $a.dst OVER follow").unwrap();
    match result {
        Statement::Go(stmt) => {
            assert!(stmt.from_clause.vids.is_empty());
            assert_eq!(stmt.from_clause.src.as_deref(), Some("a.dst"));
        }
        other => panic!("Expected Go, got {:?}", other),
    }
}

#[test]
fn test_parse_compound_assignment() {
    let result = parse("$a = GO FROM 1 OVER follow; GO FROM $a.dst OVER follow").unwrap();
    match result {
        Statement::Compound(clauses) => {
            assert_eq!(clauses.len(), 2);
            assert_eq!(clauses[0].var.as_deref(), Some("a"));
            assert!(matches!(*clauses[0].stmt, Statement::Go(_)));
            assert!(clauses[1].var.is_none());
            match *clauses[1].stmt {
                Statement::Go(ref go) => {
                    assert_eq!(go.from_clause.src.as_deref(), Some("a.dst"));
                }
                _ => panic!("expected GO as second clause"),
            }
        }
        other => panic!("Expected Compound, got {:?}", other),
    }
}

#[test]
fn test_parse_trailing_semicolon_keeps_single_statement() {
    let result = parse("GO FROM 1 OVER follow;").unwrap();
    assert!(matches!(result, Statement::Go(_)));
}

#[test]
fn test_parse_semicolon_inside_string_literal_is_not_compound_separator() {
    let result = parse(r#"INSERT VERTEX person(name) VALUES 1:("Alice; Bob")"#).unwrap();
    match result {
        Statement::Insert(stmt) => {
            assert_eq!(stmt.insert_type, InsertType::Vertex);
            assert_eq!(stmt.vertices.len(), 1);
            assert_eq!(stmt.vertices[0].tags.len(), 1);
            assert_eq!(stmt.vertices[0].tags[0].props.len(), 1);
        }
        other => panic!("Expected Insert, got {:?}", other),
    }
}

#[test]
fn test_parse_lookup() {
    let result = parse("LOOKUP ON player WHERE player.age > 30");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Lookup(stmt) => {
            match stmt.lookup_type {
                LookupType::Tag(name) => assert_eq!(name, "player"),
                _ => panic!("Expected Tag lookup"),
            }
            assert!(stmt.where_clause.is_some());
        }
        _ => panic!("Expected Lookup statement"),
    }
}

#[test]
fn test_parse_match() {
    let result = parse("MATCH (n:person) RETURN n");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Match(stmt) => {
            assert!(stmt.return_clause.is_some());
        }
        _ => panic!("Expected Match statement"),
    }
}

#[test]
fn test_parse_update_vertex() {
    let result = parse("UPDATE VERTEX ON player 1 SET age = 31");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Update(stmt) => {
            assert_eq!(stmt.update_type, UpdateType::Vertex);
            assert_eq!(stmt.tag_name, Some("player".to_string()));
            assert!(stmt.updates.contains_key("age"));
        }
        _ => panic!("Expected Update statement"),
    }
}

#[test]
fn test_parse_alter_tag_add_column() {
    let result = parse("ALTER TAG player ADD (email STRING NULL)");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Alter(AlterStatement::Tag(stmt)) => {
            assert_eq!(stmt.name, "player");
            assert_eq!(stmt.operations.len(), 1);
            match &stmt.operations[0] {
                AlterOperation::AddColumn(prop) => {
                    assert_eq!(prop.name, "email");
                    assert_eq!(prop.data_type, DataType::String);
                    assert!(prop.nullable);
                }
                _ => panic!("Expected AddColumn operation"),
            }
        }
        _ => panic!("Expected Alter Tag statement"),
    }
}

#[test]
fn test_parse_alter_tag_add_column_with_default() {
    let result = parse("ALTER TAG player ADD (score INT64 NOT NULL DEFAULT 0)");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Alter(AlterStatement::Tag(stmt)) => {
            assert_eq!(stmt.name, "player");
            assert_eq!(stmt.operations.len(), 1);
            match &stmt.operations[0] {
                AlterOperation::AddColumn(prop) => {
                    assert_eq!(prop.name, "score");
                    assert_eq!(prop.data_type, DataType::Int64);
                    assert!(!prop.nullable);
                    assert!(prop.default.is_some());
                }
                _ => panic!("Expected AddColumn operation"),
            }
        }
        _ => panic!("Expected Alter Tag statement"),
    }
}

#[test]
fn test_parse_alter_edge_add_column() {
    let result = parse("ALTER EDGE follow ADD (weight DOUBLE NULL)");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Alter(AlterStatement::Edge(stmt)) => {
            assert_eq!(stmt.name, "follow");
            assert_eq!(stmt.operations.len(), 1);
            match &stmt.operations[0] {
                AlterOperation::AddColumn(prop) => {
                    assert_eq!(prop.name, "weight");
                    assert_eq!(prop.data_type, DataType::Double);
                    assert!(prop.nullable);
                }
                _ => panic!("Expected AddColumn operation"),
            }
        }
        _ => panic!("Expected Alter Edge statement"),
    }
}

#[test]
fn test_parse_alter_tag_drop_column() {
    let result = parse("ALTER TAG player DROP (email)");
    assert!(result.is_ok(), "{:?}", result);
    match result.unwrap() {
        Statement::Alter(AlterStatement::Tag(stmt)) => {
            assert_eq!(stmt.operations.len(), 1);
            match &stmt.operations[0] {
                AlterOperation::DropColumn(name) => assert_eq!(name, "email"),
                _ => panic!("Expected DropColumn"),
            }
        }
        _ => panic!("Expected Alter Tag"),
    }
}

#[test]
fn test_parse_alter_tag_change_column() {
    let result = parse("ALTER TAG player CHANGE (age INT32 NULL)");
    assert!(result.is_ok(), "{:?}", result);
    match result.unwrap() {
        Statement::Alter(AlterStatement::Tag(stmt)) => {
            assert_eq!(stmt.operations.len(), 1);
            match &stmt.operations[0] {
                AlterOperation::ChangeColumn(prop) => {
                    assert_eq!(prop.name, "age");
                    assert_eq!(prop.data_type, DataType::Int32);
                    assert!(prop.nullable);
                }
                _ => panic!("Expected ChangeColumn"),
            }
        }
        _ => panic!("Expected Alter Tag"),
    }
}

#[test]
fn test_parse_alter_negative() {
    // Missing column details
    assert!(parse("ALTER TAG player ADD (email)").is_err());

    // Invalid keyword
    assert!(parse("ALTER SOMETHING player").is_err());

    // Not Null check (fail fast validation)
    let result = parse("ALTER TAG player ADD (score INT64 NOT NULL)");
    assert!(result.is_err());
    match result.unwrap_err() {
        ParseError::InvalidSyntax(msg) => {
            assert!(msg.contains("NOT NULL but has no DEFAULT"));
        }
        _ => panic!("Expected InvalidSyntax error"),
    }
}

#[test]
fn test_parse_create_space_with_options() {
    let result = parse("CREATE SPACE my_space (partition_num=100, replica_factor=3)");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Create(CreateStatement::Space(stmt)) => {
            assert_eq!(stmt.name, "my_space");
            assert_eq!(stmt.partition_num, Some(100));
            assert_eq!(stmt.replica_factor, Some(3));
            assert!(!stmt.if_not_exists);
        }
        _ => panic!("Expected CreateSpace"),
    }
}

#[test]
fn test_parse_create_space_with_vid_type_int64() {
    let result = parse("CREATE SPACE my_space (partition_num=10, vid_type=INT64)");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Create(CreateStatement::Space(stmt)) => {
            assert_eq!(stmt.name, "my_space");
            assert_eq!(stmt.vid_type, Some(VidType::Int64));
        }
        _ => panic!("Expected CreateSpace"),
    }
}

#[test]
fn test_parse_create_space_with_vid_type_fixed_string() {
    let result = parse("CREATE SPACE my_space (partition_num=10, vid_type=FIXED_STRING(64))");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Create(CreateStatement::Space(stmt)) => {
            assert_eq!(stmt.name, "my_space");
            assert_eq!(stmt.vid_type, Some(VidType::FixedString(64)));
        }
        _ => panic!("Expected CreateSpace"),
    }
}

#[test]
fn test_parse_create_space_partition_by_hash() {
    let result = parse("CREATE SPACE my_space (partition_num=100) PARTITION BY HASH");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Create(CreateStatement::Space(stmt)) => {
            assert_eq!(stmt.name, "my_space");
            assert_eq!(stmt.partition_num, Some(100));
            assert_eq!(stmt.partition_strategy, Some(PartitionStrategySpec::Hash));
        }
        _ => panic!("Expected CreateSpace"),
    }
}

#[test]
fn test_parse_create_space_partition_by_modulo() {
    let result = parse("CREATE SPACE my_space (partition_num=10) PARTITION BY MODULO");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Create(CreateStatement::Space(stmt)) => {
            assert_eq!(stmt.name, "my_space");
            assert_eq!(stmt.partition_strategy, Some(PartitionStrategySpec::Modulo));
        }
        _ => panic!("Expected CreateSpace"),
    }
}

#[test]
fn test_parse_create_space_partition_by_range() {
    let result = parse("CREATE SPACE my_space (partition_num=4) PARTITION BY RANGE(100, 200, 300)");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Create(CreateStatement::Space(stmt)) => {
            assert_eq!(stmt.name, "my_space");
            assert_eq!(stmt.partition_num, Some(4));
            match stmt.partition_strategy {
                Some(PartitionStrategySpec::Range { boundaries }) => {
                    assert_eq!(boundaries, vec![100, 200, 300]);
                }
                _ => panic!("Expected Range partition strategy"),
            }
        }
        _ => panic!("Expected CreateSpace"),
    }
}

#[test]
fn test_parse_create_space_full_syntax() {
    let result = parse(
        "CREATE SPACE IF NOT EXISTS test_space (partition_num=100, replica_factor=3, vid_type=INT64) PARTITION BY HASH",
    );
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Create(CreateStatement::Space(stmt)) => {
            assert_eq!(stmt.name, "test_space");
            assert!(stmt.if_not_exists);
            assert_eq!(stmt.partition_num, Some(100));
            assert_eq!(stmt.replica_factor, Some(3));
            assert_eq!(stmt.vid_type, Some(VidType::Int64));
            assert_eq!(stmt.partition_strategy, Some(PartitionStrategySpec::Hash));
        }
        _ => panic!("Expected CreateSpace"),
    }
}

#[test]
fn test_parse_show_parts() {
    let result = parse("SHOW PARTS");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Show(ShowStatement::Parts) => {}
        _ => panic!("Expected ShowParts"),
    }
}

#[test]
fn test_parse_show_hosts() {
    let result = parse("SHOW HOSTS");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Show(ShowStatement::Hosts) => {}
        _ => panic!("Expected ShowHosts"),
    }
}

#[test]
fn test_parse_balance_leader() {
    let result = parse("BALANCE LEADER");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Balance(BalanceStatement::Leader) => {}
        _ => panic!("Expected BalanceLeader"),
    }
}

#[test]
fn test_parse_balance_data() {
    let result = parse("BALANCE DATA");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Balance(BalanceStatement::Data) => {}
        _ => panic!("Expected BalanceData"),
    }
}

#[test]
fn test_parse_balance_status() {
    let result = parse("BALANCE STATUS");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Balance(BalanceStatement::Status) => {}
        _ => panic!("Expected BalanceStatus"),
    }
}

#[test]
fn test_parse_balance_stop() {
    let result = parse("BALANCE STOP");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Balance(BalanceStatement::Stop) => {}
        _ => panic!("Expected BalanceStop"),
    }
}

#[test]
fn test_parse_balance_reset() {
    let result = parse("BALANCE RESET");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Balance(BalanceStatement::Reset) => {}
        _ => panic!("Expected BalanceReset"),
    }
}

// ===== INDEX statements (parser expects: CREATE INDEX <TAG|EDGE> ...) =====

#[test]
fn test_parse_create_tag_index() {
    let result = parse("CREATE INDEX TAG person_age_idx ON TAG person(age)");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Create(CreateStatement::TagIndex(stmt)) => {
            assert_eq!(stmt.index_name, "person_age_idx");
            assert_eq!(stmt.tag_name, "person");
            assert_eq!(stmt.props, vec!["age".to_string()]);
            assert!(!stmt.if_not_exists);
        }
        s => panic!("Expected CreateTagIndex, got {:?}", s),
    }
}

#[test]
fn test_parse_create_tag_index_if_not_exists_multi_props() {
    let result = parse("CREATE INDEX TAG IF NOT EXISTS multi_idx ON TAG person(name, age, city)");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Create(CreateStatement::TagIndex(stmt)) => {
            assert!(stmt.if_not_exists);
            assert_eq!(stmt.props, vec!["name", "age", "city"]);
        }
        s => panic!("Expected CreateTagIndex, got {:?}", s),
    }
}

#[test]
fn test_parse_create_edge_index() {
    let result = parse("CREATE INDEX EDGE e_idx ON EDGE knows(since)");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Create(CreateStatement::EdgeIndex(stmt)) => {
            assert_eq!(stmt.index_name, "e_idx");
            assert_eq!(stmt.edge_name, "knows");
            assert_eq!(stmt.props, vec!["since".to_string()]);
        }
        s => panic!("Expected CreateEdgeIndex, got {:?}", s),
    }
}

#[test]
fn test_parse_drop_tag_index() {
    let result = parse("DROP INDEX TAG person_age_idx");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Drop(DropStatement::TagIndex(stmt)) => {
            assert_eq!(stmt.index_name, "person_age_idx");
            assert!(!stmt.if_exists);
        }
        s => panic!("Expected DropTagIndex, got {:?}", s),
    }
}

#[test]
fn test_parse_drop_tag_index_if_exists() {
    let result = parse("DROP INDEX TAG IF EXISTS maybe_idx");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Drop(DropStatement::TagIndex(stmt)) => {
            assert!(stmt.if_exists);
            assert_eq!(stmt.index_name, "maybe_idx");
        }
        s => panic!("Expected DropTagIndex, got {:?}", s),
    }
}

#[test]
fn test_parse_drop_edge_index() {
    let result = parse("DROP INDEX EDGE e_idx");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Drop(DropStatement::EdgeIndex(stmt)) => {
            assert_eq!(stmt.index_name, "e_idx");
        }
        s => panic!("Expected DropEdgeIndex, got {:?}", s),
    }
}

// ===== CLASS statements (O-3) =====

#[test]
fn test_parse_create_class_with_subclass_of() {
    let result = parse("CREATE CLASS dog(breed STRING) SUBCLASS OF animal, pet");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Create(CreateStatement::Class(stmt)) => {
            assert_eq!(stmt.name, "dog");
            assert!(!stmt.if_not_exists);
            assert_eq!(stmt.props.len(), 1);
            assert_eq!(stmt.superclasses, vec!["animal", "pet"]);
        }
        s => panic!("Expected CreateClass, got {:?}", s),
    }
}

#[test]
fn test_parse_create_class_without_hierarchy() {
    let result = parse("CREATE CLASS IF NOT EXISTS animal(name STRING)");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Create(CreateStatement::Class(stmt)) => {
            assert_eq!(stmt.name, "animal");
            assert!(stmt.if_not_exists);
            assert!(stmt.superclasses.is_empty());
        }
        s => panic!("Expected CreateClass, got {:?}", s),
    }
}

#[test]
fn test_parse_drop_class() {
    match parse("DROP CLASS IF EXISTS dog").unwrap() {
        Statement::Drop(DropStatement::Class(stmt)) => {
            assert_eq!(stmt.name, "dog");
            assert!(stmt.if_exists);
        }
        s => panic!("Expected DropClass, got {:?}", s),
    }
}

#[test]
fn test_parse_show_classes_and_describe_class() {
    assert!(matches!(
        parse("SHOW CLASSES").unwrap(),
        Statement::Show(ShowStatement::Classes)
    ));
    match parse("DESCRIBE CLASS dog").unwrap() {
        Statement::Describe(DescribeStatement::Class(name)) => assert_eq!(name, "dog"),
        s => panic!("Expected DescribeClass, got {:?}", s),
    }
}

// ===== FIND PATH statements =====

#[test]
fn test_parse_find_path() {
    let result = parse("FIND PATH FROM 1 TO 2 OVER knows");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Find(stmt) => {
            assert_eq!(stmt.find_type, FindType::Path);
            assert_eq!(stmt.over_edge, "knows");
            assert!(stmt.where_clause.is_none());
            assert!(stmt.yield_clause.is_none());
        }
        s => panic!("Expected Find, got {:?}", s),
    }
}

#[test]
fn test_parse_find_shortest_path() {
    let result = parse("FIND SHORTEST PATH FROM 1 TO 2 OVER knows");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Find(stmt) => {
            assert_eq!(stmt.find_type, FindType::ShortestPath);
        }
        s => panic!("Expected Find ShortestPath, got {:?}", s),
    }
}

#[test]
fn test_parse_find_shortest_path_with_weight() {
    let result = parse("FIND SHORTEST PATH FROM 1 TO 2 OVER knows WEIGHT BY cost");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Find(stmt) => {
            assert_eq!(stmt.find_type, FindType::ShortestPath);
            assert_eq!(stmt.over_edge, "knows");
            assert_eq!(stmt.weight_prop.as_deref(), Some("cost"));
        }
        s => panic!("Expected Find ShortestPath, got {:?}", s),
    }
}

#[test]
fn test_parse_find_all_shortest_paths_bidirect_upto() {
    let result = parse("FIND ALL SHORTEST PATHS FROM 1 TO 2 OVER knows BIDIRECT UPTO 5 STEPS");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Find(stmt) => {
            assert_eq!(stmt.find_type, FindType::AllShortestPaths);
            assert_eq!(stmt.over_edge, "knows");
            assert!(stmt.bidirect);
            assert_eq!(stmt.upto_steps, Some(5));
        }
        s => panic!("Expected Find AllShortestPaths, got {:?}", s),
    }
}

#[test]
fn test_parse_find_all_shortest_path_singular_keyword() {
    // ALL SHORTEST PATH (singular) is accepted as an alias of PATHS.
    let result = parse("FIND ALL SHORTEST PATH FROM 1 TO 2 OVER knows");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Find(stmt) => {
            assert_eq!(stmt.find_type, FindType::AllShortestPaths);
            assert!(!stmt.bidirect);
            assert_eq!(stmt.upto_steps, None);
        }
        s => panic!("Expected Find AllShortestPaths, got {:?}", s),
    }
}

#[test]
fn test_parse_find_shortest_path_bidirect() {
    let result = parse("FIND SHORTEST PATH FROM 1 TO 2 OVER knows BIDIRECT");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Find(stmt) => {
            assert_eq!(stmt.find_type, FindType::ShortestPath);
            assert!(stmt.bidirect);
        }
        s => panic!("Expected Find ShortestPath, got {:?}", s),
    }
}

#[test]
fn test_parse_find_upto_rejects_non_positive_steps() {
    assert!(parse("FIND SHORTEST PATH FROM 1 TO 2 OVER knows UPTO 0 STEPS").is_err());
    assert!(parse("FIND SHORTEST PATH FROM 1 TO 2 OVER knows UPTO x STEPS").is_err());
}

#[test]
fn test_parse_find_path_with_where_clause() {
    let result = parse("FIND PATH FROM 1 TO 2 OVER knows WHERE knows.since > 2020");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Find(stmt) => {
            assert!(stmt.where_clause.is_some());
        }
        s => panic!("Expected Find, got {:?}", s),
    }
}

// Note: ALTER operations currently only support ADD; DROP/CHANGE not implemented in
// parse_alter_operations(). Existing tests cover ADD with default and NOT NULL paths.

// ===== DESCRIBE / DESC tests =====

#[test]
fn test_parse_describe_tag() {
    let result = parse("DESCRIBE TAG person");
    assert!(result.is_ok(), "expected ok, got {:?}", result);
    match result.unwrap() {
        Statement::Describe(DescribeStatement::Tag(name)) => assert_eq!(name, "person"),
        other => panic!("Expected Describe(Tag), got {:?}", other),
    }
}

#[test]
fn test_parse_desc_tag_alias() {
    let result = parse("DESC TAG person");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Describe(DescribeStatement::Tag(name)) => assert_eq!(name, "person"),
        other => panic!("Expected Describe(Tag), got {:?}", other),
    }
}

#[test]
fn test_parse_describe_edge() {
    let result = parse("DESCRIBE EDGE follows");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Describe(DescribeStatement::Edge(name)) => assert_eq!(name, "follows"),
        other => panic!("Expected Describe(Edge), got {:?}", other),
    }
}

#[test]
fn test_parse_desc_edge_alias() {
    let result = parse("DESC EDGE follows");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Describe(DescribeStatement::Edge(name)) => assert_eq!(name, "follows"),
        other => panic!("Expected Describe(Edge), got {:?}", other),
    }
}

#[test]
fn test_parse_describe_space() {
    let result = parse("DESCRIBE SPACE my_space");
    assert!(result.is_ok());
    match result.unwrap() {
        Statement::Describe(DescribeStatement::Space(name)) => assert_eq!(name, "my_space"),
        other => panic!("Expected Describe(Space), got {:?}", other),
    }
}

#[test]
fn test_parse_describe_case_insensitive() {
    for q in &[
        "describe tag person",
        "Describe Tag person",
        "DESCRIBE tag PERSON",
    ] {
        let result = parse(q);
        assert!(result.is_ok(), "query failed: {}", q);
    }
}

#[test]
fn test_parse_describe_missing_name_fails() {
    let result = parse("DESCRIBE TAG");
    assert!(result.is_err(), "expected parse error for missing name");
}

#[test]
fn test_parse_describe_invalid_target_fails() {
    let result = parse("DESCRIBE USER alice");
    assert!(result.is_err(), "DESCRIBE USER should not parse");
}

// ===== DEFAULT value parsing (Item 3) =====

#[test]
fn test_parse_create_tag_with_default_int() {
    let result = parse("CREATE TAG player(score INT64 DEFAULT 0)");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Create(CreateStatement::Tag(stmt)) => {
            assert_eq!(stmt.props.len(), 1);
            assert_eq!(
                stmt.props[0].default,
                Some(Expression::Literal(Literal::Int(0)))
            );
        }
        s => panic!("Expected CreateTag, got {:?}", s),
    }
}

#[test]
fn test_parse_create_tag_with_default_string_and_null_mix() {
    let result = parse(
        "CREATE TAG person(name STRING NULL DEFAULT 'unknown', age INT64 DEFAULT -1, active BOOL DEFAULT true)",
    );
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Create(CreateStatement::Tag(stmt)) => {
            assert_eq!(stmt.props.len(), 3);
            assert!(stmt.props[0].nullable);
            assert_eq!(
                stmt.props[0].default,
                Some(Expression::Literal(Literal::String("unknown".to_string())))
            );
            assert_eq!(
                stmt.props[1].default,
                Some(Expression::Literal(Literal::Int(-1)))
            );
            assert_eq!(
                stmt.props[2].default,
                Some(Expression::Literal(Literal::Bool(true)))
            );
        }
        s => panic!("Expected CreateTag, got {:?}", s),
    }
}

#[test]
fn test_parse_create_edge_with_default_float_and_null() {
    let result = parse("CREATE EDGE knows(weight FLOAT DEFAULT 1.5, note STRING DEFAULT NULL)");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Create(CreateStatement::Edge(stmt)) => {
            assert_eq!(
                stmt.props[0].default,
                Some(Expression::Literal(Literal::Float(1.5)))
            );
            assert_eq!(
                stmt.props[1].default,
                Some(Expression::Literal(Literal::Null))
            );
        }
        s => panic!("Expected CreateEdge, got {:?}", s),
    }
}

#[test]
fn test_parse_default_rejects_non_literal() {
    // DEFAULT values must be literals — function calls / identifiers
    // are rejected so the downstream schema layer sees only storable
    // constants.
    let result = parse("CREATE TAG player(id INT64 DEFAULT now())");
    assert!(
        result.is_err(),
        "function-call default should fail to parse"
    );
}

// ===== MATCH node / edge property filter parsing (Item 4) =====

#[test]
fn test_parse_match_with_node_property_filter() {
    let result = parse("MATCH (n:person {name: 'Alice', age: 30}) RETURN n");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Match(stmt) => {
            let path = match stmt.pattern {
                Pattern::Path(p) => p,
                _ => panic!("Expected Pattern::Path"),
            };
            assert_eq!(path.start.labels, vec!["person".to_string()]);
            assert_eq!(
                path.start.props.get("name"),
                Some(&Expression::Literal(Literal::String("Alice".to_string())))
            );
            assert_eq!(
                path.start.props.get("age"),
                Some(&Expression::Literal(Literal::Int(30)))
            );
        }
        s => panic!("Expected Match, got {:?}", s),
    }
}

#[test]
fn test_parse_match_with_edge_property_filter() {
    let result = parse("MATCH (a:person)-[r:knows {since: 2020}]->(b:person) RETURN a");
    assert!(result.is_ok(), "parse failed: {:?}", result);
    match result.unwrap() {
        Statement::Match(stmt) => {
            let path = match stmt.pattern {
                Pattern::Path(p) => p,
                _ => panic!("Expected Pattern::Path"),
            };
            assert_eq!(path.edges.len(), 1);
            assert_eq!(
                path.edges[0].props.get("since"),
                Some(&Expression::Literal(Literal::Int(2020)))
            );
        }
        s => panic!("Expected Match, got {:?}", s),
    }
}

#[test]
fn test_parse_match_empty_property_block_is_allowed() {
    let result = parse("MATCH (n:person {}) RETURN n");
    assert!(result.is_ok(), "parse failed: {:?}", result);
}

#[test]
fn test_parse_match_duplicate_property_key_fails() {
    // Guards parse_pattern_properties's duplicate-key check.
    let result = parse("MATCH (n {a: 1, a: 2}) RETURN n");
    assert!(result.is_err(), "duplicate property key should be rejected");
}

#[test]
fn test_parse_match_property_value_must_be_literal() {
    // Identifiers / function calls are not accepted as property filter
    // values.
    let result = parse("MATCH (n {name: some_var}) RETURN n");
    assert!(result.is_err(), "non-literal property value should fail");
}

#[test]
fn test_go_yield_dst_vertex_prop() {
    let result = parse("GO FROM 1 OVER follows YIELD $$.person.name").unwrap();
    match result {
        Statement::Go(stmt) => {
            assert_eq!(stmt.yield_clause.columns.len(), 1);
            assert!(
                matches!(
                    &stmt.yield_clause.columns[0].expression,
                    Expression::DstVertexProp { tag, prop }
                    if tag == "person" && prop == "name"
                ),
                "expected DstVertexProp, got {:?}",
                stmt.yield_clause.columns[0].expression
            );
        }
        other => panic!("Expected Go, got {:?}", other),
    }
}

#[test]
fn test_go_yield_edge_prop_ref() {
    let result = parse("GO FROM 1 OVER works_at YIELD works_at.role").unwrap();
    match result {
        Statement::Go(stmt) => {
            assert_eq!(stmt.yield_clause.columns.len(), 1);
            assert!(
                matches!(
                    &stmt.yield_clause.columns[0].expression,
                    Expression::PropRef { object, prop }
                    if object == "works_at" && prop == "role"
                ),
                "expected PropRef, got {:?}",
                stmt.yield_clause.columns[0].expression
            );
        }
        other => panic!("Expected Go, got {:?}", other),
    }
}

#[test]
fn test_go_reversely_preserves_direction() {
    let result = parse("GO FROM 2 OVER knows REVERSELY YIELD knows.since").unwrap();
    match result {
        Statement::Go(stmt) => {
            assert_eq!(stmt.over_edges, vec!["knows".to_string()]);
            assert_eq!(stmt.direction, EdgeDirection::Incoming);
        }
        other => panic!("Expected Go, got {:?}", other),
    }
}

#[test]
fn test_parse_match_edge_variable_count() {
    let result = parse("MATCH (p:person)-[e:has_interest]->(t:tag) RETURN count(e)");
    assert!(result.is_ok(), "parse failed: {:?}", result);
}

#[test]
fn test_create_tag_index_with_string_length_hint() {
    let result = parse("CREATE TAG INDEX person_name_idx ON TAG person(name(30))").unwrap();
    match result {
        Statement::Create(CreateStatement::TagIndex(stmt)) => {
            assert_eq!(stmt.index_name, "person_name_idx");
            assert_eq!(stmt.tag_name, "person");
            assert_eq!(stmt.props, vec!["name"]);
        }
        other => panic!("Expected CreateTagIndex, got {:?}", other),
    }
}

#[test]
fn test_match_comma_multipattern_preserves_where_return_limit() {
    // H-6 regression: comma-separated multi-patterns must NOT swallow the
    // trailing pattern + WHERE/RETURN/LIMIT clauses.
    let q = "MATCH (p:bench_product)-[:bench_belongs_to]->(c:bench_category), \
             (p)-[:bench_has_tag]->(t:bench_tag) \
             WHERE id(c)==42 RETURN p.bench_product.name AS name, t.bench_tag.name AS tname LIMIT 10";
    let stmt = parse(q).expect("should parse");
    match stmt {
        Statement::Match(m) => {
            // Pattern must be Multiple with 2 paths
            match &m.pattern {
                Pattern::Multiple(ps) => assert_eq!(ps.len(), 2, "expected 2 patterns"),
                other => panic!("expected Pattern::Multiple, got {:?}", other),
            }
            // The clauses after the comma must survive
            assert!(m.where_clause.is_some(), "WHERE must be parsed");
            assert!(m.return_clause.is_some(), "RETURN must be parsed");
            assert_eq!(m.limit, Some(10), "LIMIT must be parsed");
        }
        other => panic!("expected Match, got {:?}", other),
    }
}

#[test]
fn test_match_single_pattern_still_path_not_multiple() {
    // Single pattern must remain Pattern::Path (no regression).
    let q = "MATCH (p:product)-[:belongs_to]->(c:category) WHERE id(c)==42 RETURN p LIMIT 5";
    let stmt = parse(q).expect("should parse");
    match stmt {
        Statement::Match(m) => {
            assert!(
                matches!(m.pattern, Pattern::Path(_)),
                "single pattern stays Path"
            );
            assert_eq!(m.limit, Some(5));
        }
        other => panic!("expected Match, got {:?}", other),
    }
}

#[test]
fn test_explain_sets_profile_false() {
    let stmt = parse("EXPLAIN MATCH (p:product) RETURN p").expect("should parse EXPLAIN");
    match stmt {
        Statement::Explain { profile, statement } => {
            assert!(!profile, "EXPLAIN must have profile=false");
            assert!(matches!(*statement, Statement::Match(_)));
        }
        other => panic!("expected Explain, got {:?}", other),
    }
}

#[test]
fn test_profile_sets_profile_true() {
    let stmt = parse("PROFILE GO FROM 1 OVER follow").expect("should parse PROFILE");
    match stmt {
        Statement::Explain { profile, statement } => {
            assert!(profile, "PROFILE must have profile=true");
            assert!(matches!(*statement, Statement::Go(_)));
        }
        other => panic!("expected Explain, got {:?}", other),
    }
}

#[test]
fn test_create_index_on_keyword_named_tag() {
    // LDBC's `Tag` class: the tag name collides with the TAG keyword.
    // `ON Tag(...)` must read `Tag` as the tag name, not the optional keyword.
    parse("CREATE TAG INDEX tag_name_idx ON Tag(name)").unwrap();
    parse("CREATE TAG INDEX IF NOT EXISTS tag_name_idx ON Tag(name)").unwrap();
    // Explicit `ON TAG name(...)` prefix still works.
    parse("CREATE INDEX TAG person_age_idx ON TAG person(age)").unwrap();
    // Plain `ON name(...)` (TAG keyword omitted) still works.
    parse("CREATE TAG INDEX i ON person(last_name)").unwrap();
}

#[test]
fn test_create_edge_index_on_keyword_named_edge() {
    parse("CREATE EDGE INDEX e_idx ON Edge(weight)").unwrap();
    parse("CREATE INDEX EDGE e_idx ON EDGE knows(since)").unwrap();
    parse("CREATE EDGE INDEX e_idx ON knows(since)").unwrap();
}

#[test]
fn test_parse_errors_carry_location_and_expectation() {
    // consume_identifier failure: reports what was found + where (feedback #7).
    let e = parse("CREATE TAG INDEX i ON (x)").unwrap_err();
    let m = format!("{e}");
    assert!(
        m.contains("identifier expected") && m.contains("line 1") && m.contains("column"),
        "identifier error must carry location: {m}"
    );

    // consume_token failure: reports expected + found + location.
    let e2 = parse("CREATE SPACE s PARTITION 4").unwrap_err(); // expects BY after PARTITION
    let m2 = format!("{e2}");
    assert!(
        m2.contains("expected") && m2.contains("line 1"),
        "token error must carry expected+location: {m2}"
    );
}

// ===== RECOMMEND (PLAN.md R-1) =====

#[test]
fn test_parse_recommend_basic() {
    match parse("RECOMMEND SIMILAR TO 100 OVER follows LIMIT 5").unwrap() {
        Statement::Recommend(s) => {
            assert_eq!(s.src_vid, 100);
            assert_eq!(s.limit, 5);
            match s.by {
                RecommendBy::Neighbors { over_edges, metric } => {
                    assert_eq!(over_edges, vec!["follows".to_string()]);
                    assert_eq!(metric, SimilarityMetric::Jaccard);
                }
                other => panic!("Expected Neighbors, got {:?}", other),
            }
        }
        other => panic!("Expected Recommend, got {:?}", other),
    }
}

#[test]
fn test_parse_recommend_multi_edge_default_limit() {
    match parse("RECOMMEND SIMILAR TO 1 OVER has_brand, in_category").unwrap() {
        Statement::Recommend(s) => {
            assert_eq!(s.limit, 10); // default
            match s.by {
                RecommendBy::Neighbors { over_edges, .. } => assert_eq!(
                    over_edges,
                    vec!["has_brand".to_string(), "in_category".to_string()]
                ),
                other => panic!("Expected Neighbors, got {:?}", other),
            }
        }
        other => panic!("Expected Recommend, got {:?}", other),
    }
}

#[test]
fn test_parse_recommend_star_means_all_edges() {
    match parse("RECOMMEND SIMILAR TO 7 OVER *").unwrap() {
        Statement::Recommend(s) => match s.by {
            RecommendBy::Neighbors { over_edges, .. } => assert!(over_edges.is_empty()),
            other => panic!("Expected Neighbors, got {:?}", other),
        },
        other => panic!("Expected Recommend, got {:?}", other),
    }
}

#[test]
fn test_parse_recommend_rejects_zero_limit() {
    assert!(parse("RECOMMEND SIMILAR TO 1 OVER e LIMIT 0").is_err());
}

#[test]
fn test_parse_recommend_by_embedding() {
    match parse("RECOMMEND SIMILAR TO 42 BY EMBEDDING vec LIMIT 8").unwrap() {
        Statement::Recommend(s) => {
            assert_eq!(s.src_vid, 42);
            assert_eq!(s.limit, 8);
            match s.by {
                RecommendBy::Embedding { prop } => assert_eq!(prop, "vec"),
                other => panic!("Expected Embedding, got {:?}", other),
            }
        }
        other => panic!("Expected Recommend, got {:?}", other),
    }
}

#[test]
fn test_parse_recommend_embedding_default_limit() {
    match parse("RECOMMEND SIMILAR TO 1 BY EMBEDDING embedding").unwrap() {
        Statement::Recommend(s) => {
            assert_eq!(s.limit, 10);
            assert!(matches!(s.by, RecommendBy::Embedding { .. }));
        }
        other => panic!("Expected Recommend, got {:?}", other),
    }
}

#[test]
fn test_parse_recommend_rejects_unknown_mode() {
    assert!(parse("RECOMMEND SIMILAR TO 1 FROM x").is_err());
}

#[test]
fn test_parse_recommend_with_where_filter() {
    match parse("RECOMMEND SIMILAR TO 1 BY EMBEDDING emb WHERE channel = \"coupang\" LIMIT 5")
        .unwrap()
    {
        Statement::Recommend(s) => {
            assert_eq!(s.limit, 5);
            assert!(s.filter.is_some(), "WHERE filter should be captured");
            assert!(matches!(s.by, RecommendBy::Embedding { .. }));
        }
        other => panic!("Expected Recommend, got {:?}", other),
    }
}

#[test]
fn test_parse_recommend_neighbors_with_where() {
    match parse("RECOMMEND SIMILAR TO 1 OVER knows WHERE channel = \"x\"").unwrap() {
        Statement::Recommend(s) => {
            assert!(s.filter.is_some());
            assert!(matches!(s.by, RecommendBy::Neighbors { .. }));
        }
        other => panic!("Expected Recommend, got {:?}", other),
    }
}

#[test]
fn test_parse_create_class_disjoint() {
    match parse("CREATE CLASS person() DISJOINT WITH building, vehicle").unwrap() {
        Statement::Create(CreateStatement::Class(c)) => {
            assert_eq!(
                c.disjoint,
                vec!["building".to_string(), "vehicle".to_string()]
            );
        }
        other => panic!("Expected Create Class, got {:?}", other),
    }
}

#[test]
fn test_parse_check_consistency() {
    assert!(matches!(
        parse("CHECK CONSISTENCY").unwrap(),
        Statement::CheckConsistency
    ));
}

#[test]
fn test_parse_why_inference() {
    match parse("WHY 1 -> 3 OVER ancestor").unwrap() {
        Statement::ExplainInference {
            src,
            dst,
            edge_type,
        } => {
            assert_eq!(src, 1);
            assert_eq!(dst, 3);
            assert_eq!(edge_type, "ancestor");
        }
        other => panic!("expected ExplainInference, got {other:?}"),
    }
}

#[test]
fn test_parse_create_edge_semantics() {
    match parse("CREATE EDGE ancestor() TRANSITIVE").unwrap() {
        Statement::Create(CreateStatement::Edge(e)) => {
            assert!(e.semantics.transitive);
            assert!(!e.semantics.symmetric);
        }
        other => panic!("Expected Create Edge, got {:?}", other),
    }
    match parse("CREATE EDGE child() INVERSE OF parent SUBPROPERTY OF related").unwrap() {
        Statement::Create(CreateStatement::Edge(e)) => {
            assert_eq!(e.semantics.inverse_of.as_deref(), Some("parent"));
            assert_eq!(e.semantics.subproperty_of.as_deref(), Some("related"));
        }
        other => panic!("Expected Create Edge, got {:?}", other),
    }
    match parse("CREATE EDGE bornIn() DOMAIN person RANGE city").unwrap() {
        Statement::Create(CreateStatement::Edge(e)) => {
            assert_eq!(e.semantics.domain.as_deref(), Some("person"));
            assert_eq!(e.semantics.range.as_deref(), Some("city"));
        }
        other => panic!("Expected Create Edge, got {:?}", other),
    }
    match parse("CREATE EDGE grandparent() CHAIN parent, parent").unwrap() {
        Statement::Create(CreateStatement::Edge(e)) => {
            assert_eq!(
                e.semantics.property_chain.as_deref(),
                Some(["parent".to_string(), "parent".to_string()].as_slice())
            );
        }
        other => panic!("Expected Create Edge, got {:?}", other),
    }
    // No semantic clause → all default off.
    match parse("CREATE EDGE plain()").unwrap() {
        Statement::Create(CreateStatement::Edge(e)) => assert!(e.semantics.is_empty()),
        other => panic!("Expected Create Edge, got {:?}", other),
    }
}

#[test]
fn test_parse_recommend_blend() {
    match parse(
        "RECOMMEND SIMILAR TO 1 BLEND EMBEDDING emb 0.7 OVER has_brand, in_category 0.3 LIMIT 5",
    )
    .unwrap()
    {
        Statement::Recommend(s) => {
            assert_eq!(s.limit, 5);
            match s.by {
                RecommendBy::Blend {
                    embedding_prop,
                    embedding_weight,
                    over_edges,
                    structural_weight,
                } => {
                    assert_eq!(embedding_prop, "emb");
                    assert!((embedding_weight - 0.7).abs() < 1e-9);
                    assert_eq!(
                        over_edges,
                        vec!["has_brand".to_string(), "in_category".to_string()]
                    );
                    assert!((structural_weight - 0.3).abs() < 1e-9);
                }
                other => panic!("Expected Blend, got {:?}", other),
            }
        }
        other => panic!("Expected Recommend, got {:?}", other),
    }
}

#[test]
fn test_parse_recommend_blend_rejects_negative_weight() {
    assert!(parse("RECOMMEND SIMILAR TO 1 BLEND EMBEDDING emb -0.5 OVER e 0.3").is_err());
}

#[test]
fn test_parse_insert_vector_literal() {
    // List literal with negatives — the embedding ingestion path.
    let r = parse("INSERT VERTEX product(emb) VALUES 1:([0.1, -0.2, 0.3])");
    assert!(r.is_ok(), "vector literal should parse: {:?}", r);
}

#[test]
fn test_parse_empty_list_literal() {
    let r = parse("INSERT VERTEX product(tags) VALUES 1:([])");
    assert!(r.is_ok(), "empty list should parse: {:?}", r);
}

#[test]
fn test_unquote_interprets_escapes() {
    // Bare value, no escapes (fast path).
    assert_eq!(unquote(r#""hello""#), "hello");
    // A single quote needs no escape inside a double-quoted value.
    assert_eq!(unquote(r#""Chef's""#), "Chef's");
    // Escaped delimiter quote, backslash, and a control escape.
    assert_eq!(unquote(r#""a\"b""#), "a\"b");
    assert_eq!(unquote(r#""C:\\path""#), "C:\\path");
    assert_eq!(unquote(r#""line\nbreak""#), "line\nbreak");
    // Single-quoted string with an escaped single quote.
    assert_eq!(unquote(r"'it\'s'"), "it's");
    // Unknown escape drops the backslash, keeps the char.
    assert_eq!(unquote(r#""x\qy""#), "xqy");
}

#[test]
fn test_parse_string_with_quotes_and_backslash() {
    // Dogfooding gap (nexprice load): product/brand names with apostrophes or
    // quotes (KIEHL'S, 6\" pan, C:\dir) must parse. The old `[^"]*` lexer regex
    // truncated the token at the inner quote/backslash → "Unexpected end of
    // input". All three must now parse cleanly.
    assert!(parse(r#"INSERT VERTEX p(name) VALUES 1:("Chef's")"#).is_ok());
    assert!(parse(r#"INSERT VERTEX p(name) VALUES 1:("a\"b")"#).is_ok());
    assert!(parse(r#"INSERT VERTEX p(name) VALUES 1:("C:\\dir")"#).is_ok());
}
