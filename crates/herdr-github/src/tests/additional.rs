use super::*;
use crate::{
    Account,
    device::{
        InstallationOrganizations, merge_organizations, organization_coverage, organizations,
    },
};

fn pasted_profile() -> Profile {
    Profile {
        login: "fabien_microsoft".into(),
        avatar_url: String::new(),
        token: Arc::new("github_pat_fixture".into()),
        source: Default::default(),
        organizations: Some(vec!["emu-org".into()]),
        access: Default::default(),
    }
}

fn finish(auth: &mut Auth) {
    let reply = auth
        .incoming
        .take()
        .unwrap()
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    deliver(auth, reply);
}

fn additional() -> Auth {
    Auth::for_account(Account::additional("0123456789abcdef0123456789abcdef").unwrap())
}

#[test]
fn a_pasted_token_connects_once_github_accepted_it() {
    let mut auth = additional();
    auth.use_token_with(Store::Keyring, || Ok(Reply::Pasted(pasted_profile())));
    assert!(auth.busy());
    assert_eq!(
        auth.message.as_deref(),
        Some("Checking the token with GitHub...")
    );
    // The worker may save the token, so dismissing the page waits for it.
    auth.cancel();
    finish(&mut auth);
    assert!(auth.poll_with_store(|_| panic!("the worker already saved the token")));
    assert!(auth.connected());
    assert!(!auth.failed);
    assert!(auth.message.is_none());
    assert!(auth.can_sign_out());
    assert!(!auth.github_cli);
    assert!(auth.next_session_check.is_some());
    assert_eq!(
        auth.profile.as_ref().unwrap().organizations.as_deref(),
        Some(&["emu-org".to_owned()][..])
    );
}

#[test]
fn a_refused_token_is_shown_and_leaves_nothing_saved() {
    let mut auth = additional();
    auth.use_token_with(Store::Keyring, || Err(Error::Authentication));
    finish(&mut auth);
    auth.poll_with_store(|_| panic!("nothing to save"));
    assert!(!auth.connected());
    assert!(auth.failed);
    assert!(!auth.can_sign_out());
    assert_eq!(
        auth.message.as_deref(),
        Some(Error::Authentication.to_string().as_str())
    );
}

#[test]
fn a_token_with_nowhere_to_be_saved_is_refused_before_any_request() {
    let mut auth = additional();
    auth.use_token_with(Store::Environment, || {
        panic!("no worker without a credential store")
    });
    assert!(!auth.busy());
    assert!(auth.failed);
    assert!(
        auth.message
            .as_deref()
            .unwrap()
            .contains("credential store")
    );
}

#[test]
fn a_connected_account_does_not_take_another_token() {
    let mut auth = Auth::connected_fixture();
    auth.use_token_with(Store::Keyring, || panic!("already signed in"));
    assert!(!auth.busy());
    assert!(auth.connected());
}

#[test]
fn organizations_are_read_bounded_and_checked() {
    let value: Value = serde_json::json!({"data": {"viewer": {"organizations": {"nodes": [
        {"login": "emu-org"},
        {"login": "not an owner"},
        {"login": null},
        {"login": "Second_Org"},
    ]}}}});
    assert_eq!(organizations(&value), ["emu-org", "Second_Org"]);
    assert!(organizations(&serde_json::json!({"data": null})).is_empty());
    let many: Vec<Value> = (0..150)
        .map(|index| serde_json::json!({"login": format!("org-{index}")}))
        .collect();
    let value = serde_json::json!({"data": {"viewer": {"organizations": {"nodes": many}}}});
    assert_eq!(organizations(&value).len(), 100);
}

#[test]
fn memberships_and_private_organization_installations_are_merged() {
    assert_eq!(
        merge_organizations(
            Some(vec!["public-org".into(), "SHARED".into()]),
            Some(vec!["private-org".into(), "shared".into()])
        ),
        Some(vec![
            "public-org".into(),
            "SHARED".into(),
            "private-org".into()
        ])
    );
    assert_eq!(
        merge_organizations(Some(Vec::new()), Some(vec!["private-org".into()])),
        Some(vec!["private-org".into()])
    );
    assert_eq!(merge_organizations(None, None), None);

    // Either source failing keeps the coverage unknown, so a profile refresh
    // retains the last complete union instead of dropping owners.
    assert_eq!(
        organization_coverage(
            None,
            InstallationOrganizations::Listed(vec!["private-org".into()])
        ),
        None
    );
    assert_eq!(
        organization_coverage(
            Some(vec!["public-org".into()]),
            InstallationOrganizations::Unknown
        ),
        None
    );
}

#[test]
fn the_merged_organization_bound_keeps_both_source_pages() {
    let memberships: Vec<String> = (0..100).map(|index| format!("member-{index}")).collect();
    let installed: Vec<String> = (0..100).map(|index| format!("installed-{index}")).collect();
    let expected = memberships.len() + installed.len();
    let merged = merge_organizations(Some(memberships), Some(installed)).unwrap();
    assert_eq!(merged.len(), expected);
    assert!(merged.iter().any(|owner| owner == "installed-99"));
}

#[test]
fn a_private_organization_installation_routes_to_the_additional_account() {
    let main = Profile::fixture("main", Arc::new("main-token".into()), &[]);
    let mut additional = Profile::fixture("other", Arc::new("ghu_app-token".into()), &[]);
    additional.organizations =
        merge_organizations(Some(Vec::new()), Some(vec!["private-org".into()]));
    let tokens = crate::Tokens::new(&main, [(&additional, &[][..])]);
    assert_eq!(
        tokens.for_owner("Private-Org").expose_secret(),
        "ghu_app-token"
    );
}

#[test]
fn signing_out_an_additional_account_says_what_reads_its_repositories() {
    let mut auth = additional();
    auth.profile = Auth::connected_fixture().profile;
    auth.sign_out();
    assert!(auth.poll_with_store(|_| Ok(())));
    finish(&mut auth);
    auth.poll_with_store(|_| Ok(()));
    assert!(
        auth.message
            .as_deref()
            .unwrap()
            .contains("Its repositories use your main account")
    );
}

#[test]
fn a_check_that_cannot_list_organizations_keeps_those_listed_before() {
    let mut auth = additional();
    auth.initialized = true;
    auth.profile = Some(pasted_profile());
    let unlisted = || Profile {
        organizations: None,
        access: Default::default(),
        ..pasted_profile()
    };
    auth.complete_profile_fixture(Ok(Some(unlisted())));
    auth.poll_with_store(|_| panic!("no storage"));
    assert_eq!(
        auth.profile.as_ref().unwrap().organizations.as_deref(),
        Some(&["emu-org".to_owned()][..])
    );
    // A listing that succeeded replaces them, even when it is empty.
    auth.complete_profile_fixture(Ok(Some(Profile {
        organizations: Some(Vec::new()),
        access: Default::default(),
        ..pasted_profile()
    })));
    auth.poll_with_store(|_| panic!("no storage"));
    assert_eq!(
        auth.profile.as_ref().unwrap().organizations.as_deref(),
        Some(&[][..])
    );
    // Another login's check never inherits this account's organizations.
    auth.profile = Some(pasted_profile());
    auth.complete_profile_fixture(Ok(Some(Profile {
        login: "someone_else".into(),
        ..unlisted()
    })));
    auth.poll_with_store(|_| panic!("no storage"));
    assert!(auth.profile.as_ref().unwrap().organizations.is_none());
}
