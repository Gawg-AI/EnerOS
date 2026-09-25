    #[test]
    fn review70_reject_reused_signed_envelope_for_different_case() {
        let mut f = CandidateFixture::new(&[("CORE-70-R02", "L2")], &[], fixture_key());
        assert!(f.check().is_empty());
        f.index.cases[1].envelope = f.index.cases[0].envelope.clone();
        f.index.cases[1].envelope_sha256 = f.index.cases[0].envelope_sha256.clone();
        let (lines, violations) = f.run();
        assert!(!violations.is_empty(), "Reused R01 signed envelope for R02 was accepted: {lines:?}");
    }

    #[test]
    fn review70_reject_failed_run_record_under_pass_envelope() {
        use ed25519_dalek::Signer;
        let mut f = CandidateFixture::new(&[], &[], fixture_key());
        assert!(f.check().is_empty());
        let disk = f.root.join(&f.index.cases[0].envelope);
        let mut env: EvidenceEnvelope = read_json(&disk).unwrap();
        let rel = "failed.run-record.json";
        fs::write(f.root.join(rel), serde_json::to_vec(&serde_json::json!({
            "schema": "eneros.gate-run-record.v1",
            "case_id": "CORE-70-R01", "gate_id": "VOL70.1-GATE",
            "exit_code": 1, "verdict": "FAIL", "verdict_lines": ["VOL70.1-GATE: FAIL"],
            "ran_artifact_sha256": f.lineage.artifact_sha256,
        })).unwrap()).unwrap();
        env.artifacts = vec![ArtifactRef { name: "run-record".into(), path: rel.into(), sha256: hash_file(&f.root.join(rel)).unwrap() }];
        env.signature.clear();
        env.signature = hex(&f.key.sign(&serde_json::to_vec(&env).unwrap()).to_bytes());
        fs::write(&disk, serde_json::to_vec_pretty(&env).unwrap()).unwrap();
        f.index.cases[0].envelope_sha256 = hash_file(&disk).unwrap();
        f.index.cases[0].tested_sha256 = Some(f.lineage.artifact_sha256.clone());
        let (lines, violations) = f.run();
        assert!(!violations.is_empty(), "Failed run-record was accepted: {lines:?}");
    }
