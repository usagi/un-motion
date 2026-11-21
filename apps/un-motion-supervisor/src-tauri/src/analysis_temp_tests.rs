use super::*;

fn pending() -> PendingRuntimeAnalysisExtras {
    let root=std::env::temp_dir().join(format!("un-motion-analysis-extras-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir(&root).unwrap();
    let child=root.join("capturer");fs::create_dir(&child).unwrap();
    fs::write(child.join("frame.png"),b"synthetic image bytes").unwrap();
    PendingRuntimeAnalysisExtras{owned_root:root,capturers:vec![PendingCapturerAnalysisExtras{bind_addr:"127.0.0.1:12345".parse().unwrap(),temp_dir:child,response:serde_json::json!({})}]}
}

#[test]
fn cancel_removes_only_owned_diagnostics_but_failure_retains_recovery() {
    let a=pending();let a_root=a.owned_root.clone();let b=pending();
    let duration=SnapshotDuration::from_request(None,None);
    assert_eq!(complete_snapshot_analysis(Ok(None),Some(a),duration).unwrap(),None);
    assert!(!a_root.exists());assert!(b.owned_root.join("capturer/frame.png").exists());
    let b_root=b.owned_root.clone();
    let error=complete_snapshot_analysis(Err("disk denied".into()),Some(b),duration).unwrap_err();
    assert!(error.contains(&b_root.display().to_string()));
    assert!(b_root.join("capturer/frame.png").exists());
    cleanup_analysis_temp(&PendingRuntimeAnalysisExtras{owned_root:b_root,capturers:vec![]}).unwrap();
}

#[test]
fn successful_diagnostics_move_survives_temporary_cleanup() {
    let input=pending();let root=input.owned_root.clone();
    // Independent owned output folder stands in for the user-selected destination.
    let output=pending();let path=output.owned_root.join("record.unmf.jsonl");fs::write(&path,b"{}\n").unwrap();
    let result=complete_snapshot_analysis(Ok(Some(path.display().to_string())),Some(input),SnapshotDuration::from_request(None,None)).unwrap();
    assert_eq!(result.as_deref(),Some(path.to_str().unwrap()));assert!(!root.exists());
    assert_eq!(fs::read(output.owned_root.join("record.unmf.jsonl.analysis-assets/capturer-0-12345/frame.png")).unwrap(),b"synthetic image bytes");
    assert!(output.owned_root.join("record.unmf.jsonl.analysis-extras.jsonl").exists());
    assert_eq!(fs::read(&path).unwrap(),b"{}\n");
    cleanup_analysis_temp(&output).unwrap();
}

#[test]
fn supplementary_save_failure_keeps_primary_and_temporary_images() {
    let input=pending();let input_root=input.owned_root.clone();
    let output=pending();let path=output.owned_root.join("record.unmf.jsonl");fs::write(&path,b"{}\n").unwrap();
    fs::write(output.owned_root.join("record.unmf.jsonl.analysis-assets"),b"occupied").unwrap();
    let error=complete_snapshot_analysis(Ok(Some(path.display().to_string())),Some(input),SnapshotDuration::from_request(None,None)).unwrap_err();
    assert!(error.contains("Motion recording saved to"));assert!(error.contains(&input_root.display().to_string()));
    assert_eq!(fs::read(&path).unwrap(),b"{}\n");assert!(input_root.join("capturer/frame.png").exists());
    cleanup_analysis_temp(&PendingRuntimeAnalysisExtras{owned_root:input_root,capturers:vec![]}).unwrap();
    cleanup_analysis_temp(&output).unwrap();
}

#[test]
fn cleanup_refuses_nested_or_unowned_directory() {
    let owner=pending();let nested=owner.owned_root.join("capturer");
    assert!(cleanup_analysis_temp(&PendingRuntimeAnalysisExtras{owned_root:nested.clone(),capturers:vec![]}).is_err());
    assert!(nested.join("frame.png").exists());cleanup_analysis_temp(&owner).unwrap();
}
