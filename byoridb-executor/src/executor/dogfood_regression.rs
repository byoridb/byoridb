//! Regression tests for query-correctness bugs surfaced by nexprice dogfooding
//! (2026-06-23). Each guards a distinct fix:
//!
//! - **Bug A** — `MATCH (n:tag) WHERE id(n)==X` must honour the node label even
//!   on the bound-id fast path (previously matched a vertex of any tag).
//! - **Bug B** — `FETCH PROP ON tag <vid>` must verify tag membership
//!   (previously returned the blob of a vertex carrying only other tags).
//! - **Bug C** — `GO ... OVER *` must bind the edge accessors `type(edge)` /
//!   `dst(edge)` / `src(edge)` / `edge` (previously NULL under `OVER *`).
//! - **#2** — `RETURN expr, COUNT(*)` without an explicit `GROUP BY` must
//!   implicitly group by the non-aggregate columns (previously collapsed to one
//!   row with the first value).

#[cfg(test)]
mod tests {
    use crate::context::ExecutionContext;
    use crate::executor::Executor;
    use byoridb_kvstore::store::MemoryKVStore;
    use std::sync::Arc;

    fn create_executor() -> Executor {
        let kv = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(ExecutionContext::new(kv).with_space("default".to_string()));
        Executor::new(ctx)
    }

    async fn run(executor: &Executor, q: &str) -> crate::executor::ExecutorResult {
        let stmt = byoridb_parser::parse(q).expect("parse");
        let plan = crate::ExecutionPlanBuilder::build(stmt).expect("plan build");
        executor
            .execute(plan)
            .await
            .unwrap_or_else(|e| panic!("query failed: {q}\n{e:?}"))
    }

    /// Bug A: a bound `id(n)==X` must still satisfy the start node's label.
    #[tokio::test]
    async fn match_id_filter_respects_node_label() {
        let e = create_executor();
        run(&e, "CREATE TAG product(channel STRING)").await;
        run(&e, "CREATE TAG sku(code STRING)").await;
        run(&e, "INSERT VERTEX product(channel) VALUES 1:(\"gmarket\")").await;
        run(&e, "INSERT VERTEX sku(code) VALUES 2:(\"S1\")").await;

        let r = run(&e, "MATCH (n:product) WHERE id(n)==1 RETURN id(n) AS n").await;
        assert_eq!(r.rows.len(), 1, "product vid matches product label");

        let r = run(&e, "MATCH (n:product) WHERE id(n)==2 RETURN id(n) AS n").await;
        assert_eq!(r.rows.len(), 0, "sku vid must NOT match product label");

        let r = run(&e, "MATCH (n:sku) WHERE id(n)==2 RETURN id(n) AS n").await;
        assert_eq!(r.rows.len(), 1, "sku vid matches sku label");
    }

    /// Bug B: FETCH PROP ON <tag> only returns vertices carrying that tag.
    #[tokio::test]
    async fn fetch_prop_respects_tag_membership() {
        let e = create_executor();
        run(&e, "CREATE TAG product(channel STRING)").await;
        run(&e, "CREATE TAG sku(code STRING)").await;
        run(&e, "INSERT VERTEX product(channel) VALUES 1:(\"gmarket\")").await;
        run(&e, "INSERT VERTEX sku(code) VALUES 2:(\"S1\")").await;

        let r = run(&e, "FETCH PROP ON product 1").await;
        assert_eq!(r.rows.len(), 1, "product vid fetched under product tag");

        let r = run(&e, "FETCH PROP ON product 2").await;
        assert_eq!(r.rows.len(), 0, "sku vid must NOT fetch under product tag");
    }

    /// Bug C: GO OVER * binds edge accessor functions instead of NULL.
    #[tokio::test]
    async fn go_over_all_binds_edge_accessors() {
        let e = create_executor();
        run(&e, "CREATE TAG t(name STRING)").await;
        run(&e, "CREATE EDGE rel()").await;
        run(&e, "INSERT VERTEX t(name) VALUES 1:(\"a\")").await;
        run(&e, "INSERT VERTEX t(name) VALUES 2:(\"b\")").await;
        run(&e, "INSERT EDGE rel() VALUES 1->2:()").await;

        let r = run(
            &e,
            "GO FROM 1 OVER * YIELD type(edge) AS ty, dst(edge) AS d, src(edge) AS s",
        )
        .await;
        assert_eq!(r.rows.len(), 1);
        assert_eq!(
            r.rows[0][0],
            byoridb_common::Value::String("rel".to_string()),
            "type(edge) must be the edge type, not NULL"
        );
        assert_eq!(r.rows[0][1], byoridb_common::Value::Int(2), "dst(edge)");
        assert_eq!(r.rows[0][2], byoridb_common::Value::Int(1), "src(edge)");
    }

    /// #2: RETURN expr, COUNT(*) groups by the non-aggregate column implicitly.
    #[tokio::test]
    async fn match_return_aggregate_implicitly_groups() {
        let e = create_executor();
        run(&e, "CREATE TAG p(ch STRING)").await;
        run(&e, "INSERT VERTEX p(ch) VALUES 1:(\"a\")").await;
        run(&e, "INSERT VERTEX p(ch) VALUES 2:(\"a\")").await;
        run(&e, "INSERT VERTEX p(ch) VALUES 3:(\"b\")").await;

        let r = run(&e, "MATCH (n:p) RETURN n.p.ch AS ch, COUNT(*) AS c").await;
        assert_eq!(
            r.rows.len(),
            2,
            "grouped by channel, not collapsed to 1 row"
        );

        let mut counts = std::collections::HashMap::new();
        for row in &r.rows {
            if let (byoridb_common::Value::String(ch), byoridb_common::Value::Int(c)) =
                (&row[0], &row[1])
            {
                counts.insert(ch.clone(), *c);
            }
        }
        assert_eq!(counts.get("a"), Some(&2), "channel a has 2");
        assert_eq!(counts.get("b"), Some(&1), "channel b has 1");
    }

    /// ORDER BY: previously parsed-and-discarded (no-op) so TOP-K returned
    /// arbitrary rows. Must actually sort projected results before LIMIT.
    #[tokio::test]
    async fn match_order_by_sorts_aggregate_and_topk() {
        let e = create_executor();
        run(&e, "CREATE TAG p(ch STRING)").await;
        // counts: a=3, b=1, c=2
        for (vid, ch) in [(1, "a"), (2, "a"), (3, "a"), (4, "b"), (5, "c"), (6, "c")] {
            run(&e, &format!("INSERT VERTEX p(ch) VALUES {vid}:(\"{ch}\")")).await;
        }

        // ORDER BY count DESC → a(3), c(2), b(1)
        let r = run(
            &e,
            "MATCH (n:p) RETURN n.p.ch AS ch, COUNT(*) AS cnt GROUP BY n.p.ch ORDER BY cnt DESC",
        )
        .await;
        let order: Vec<String> = r
            .rows
            .iter()
            .filter_map(|row| match &row[0] {
                byoridb_common::Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(order, vec!["a", "c", "b"], "descending by count");

        // TOP-1
        let r = run(
            &e,
            "MATCH (n:p) RETURN n.p.ch AS ch, COUNT(*) AS cnt GROUP BY n.p.ch ORDER BY cnt DESC LIMIT 1",
        )
        .await;
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.rows[0][0], byoridb_common::Value::String("a".to_string()));

        // ASC → b(1) first
        let r = run(
            &e,
            "MATCH (n:p) RETURN n.p.ch AS ch, COUNT(*) AS cnt GROUP BY n.p.ch ORDER BY cnt ASC",
        )
        .await;
        assert_eq!(r.rows[0][0], byoridb_common::Value::String("b".to_string()));
    }

    /// ORDER BY on a plain (non-aggregate) projection + LIMIT.
    #[tokio::test]
    async fn match_order_by_plain_projection() {
        let e = create_executor();
        run(&e, "CREATE TAG p(ch STRING)").await;
        for vid in 1..=6 {
            run(&e, &format!("INSERT VERTEX p(ch) VALUES {vid}:(\"x\")")).await;
        }
        // id(n) DESC LIMIT 3 → 6, 5, 4
        let r = run(
            &e,
            "MATCH (n:p) RETURN id(n) AS vid ORDER BY vid DESC LIMIT 3",
        )
        .await;
        let vids: Vec<i64> = r
            .rows
            .iter()
            .filter_map(|row| match &row[0] {
                byoridb_common::Value::Int(i) => Some(*i),
                _ => None,
            })
            .collect();
        assert_eq!(vids, vec![6, 5, 4], "descending vids, top-3");
    }
}
