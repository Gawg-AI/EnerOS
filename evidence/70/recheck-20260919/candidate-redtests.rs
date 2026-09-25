
    #[test]
    fn recheck70_subgate_cannot_prove_parent_required_case() {
        let mut f = CandidateFixture::new(&[("CORE-70-R02", "L2")], &[], fixture_key());
        assert!(f.check().is_empty());
        f.matrix.cases.iter_mut().find(|c| c.case_id == "CORE-70-R02").unwrap().gates = vec!["VOL70.2".into()];
        f.retarget_run_record("CORE-70-R02", serde_json::json!({
            "gate_id":"VOL70.2", "verdict_lines":["VOL70.2-R03-POWER-CUT GATE: PASS"]
        }));
        let (lines, violations) = f.run();
        assert!(!violations.is_empty(), "R03-only result passed R02: {lines:?}");
    }
    #[test]
    fn recheck70_malformed_pass_token_is_not_terminal_pass() {
        assert_ne!(gate_terminal_status("VOL70.1-GATE: PASSED_NOT", "VOL70.1"),Some(true));
    }
