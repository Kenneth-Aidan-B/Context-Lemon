use lemonade_context_engine_lib::rag;

mod common;

/// Requires a running Lemonade server at localhost:13305. The fixture indexes the
/// bundled sample into disposable storage, keeping the test isolated from the user's
/// application index. It skips only when Lemonade itself is unreachable.
#[tokio::test]
async fn known_answer_questions_are_grounded_with_citations() {
    let Some(fixture) = common::setup().await else {
        return;
    };

    let cases = [
        ("What port does the Nightingale gateway listen on by default?", "7913"),
        ("Who is the lead engineer on Project Nightingale?", "Priya"),
        ("How long can a device be offline before Nightingale falls back to cloud inference?", "90"),
    ];

    for (question, expected_fact) in cases {
        let response = rag::ask(&fixture.store, &fixture.client, question)
            .await
            .unwrap_or_else(|e| panic!("ask() failed for {question:?}: {e}"));

        assert!(
            !response.sources.is_empty(),
            "expected at least one citation for {question:?}, got none"
        );
        assert!(
            response.answer.contains(expected_fact),
            "expected answer to {question:?} to mention {expected_fact:?}, got: {}",
            response.answer
        );
    }
}
