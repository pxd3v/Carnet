use std::fs;

use carnet::catalog::{Catalog, CatalogError};
use tempfile::tempdir;

#[test]
fn saves_and_loads_a_default_registration_with_a_canonical_path() {
    let sandbox = tempdir().unwrap();
    let repo = sandbox.path().join("notes");
    fs::create_dir(&repo).unwrap();
    let config_path = sandbox.path().join("catalog.toml");

    let mut catalog = Catalog::create_at(&config_path);
    let registered = catalog.register("personal", &repo).unwrap();
    catalog.save().unwrap();

    let reloaded = Catalog::load_at(&config_path).unwrap();
    assert_eq!(reloaded.resolve_repo(None).unwrap(), registered);
    assert_eq!(
        reloaded.resolve_repo(None).unwrap().path,
        fs::canonicalize(repo).unwrap()
    );
}

#[test]
fn rejects_a_duplicate_registration_name() {
    let sandbox = tempdir().unwrap();
    let repo = sandbox.path().join("notes");
    fs::create_dir(&repo).unwrap();
    let mut catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    catalog.register("personal", &repo).unwrap();

    let error = catalog.register("personal", &repo).unwrap_err();

    assert!(matches!(error, CatalogError::DuplicateName { .. }));
}

#[test]
fn rejects_registration_of_a_missing_path() {
    let sandbox = tempdir().unwrap();
    let missing = sandbox.path().join("missing");
    let mut catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));

    let error = catalog.register("personal", &missing).unwrap_err();

    assert!(matches!(error, CatalogError::RepositoryPathMissing { .. }));
}

#[test]
fn replaces_the_existing_catalog_when_saving() {
    use std::os::unix::fs::MetadataExt;

    let sandbox = tempdir().unwrap();
    let repo = sandbox.path().join("notes");
    fs::create_dir(&repo).unwrap();
    let config_path = sandbox.path().join("catalog.toml");
    fs::write(&config_path, "not valid toml = [").unwrap();
    let original_inode = fs::metadata(&config_path).unwrap().ino();

    let mut catalog = Catalog::create_at(&config_path);
    catalog.register("personal", &repo).unwrap();
    catalog.save().unwrap();

    assert!(Catalog::load_at(&config_path).is_ok());
    assert!(
        !fs::read_to_string(&config_path)
            .unwrap()
            .contains("not valid")
    );
    assert_ne!(fs::metadata(&config_path).unwrap().ino(), original_inode);
}

#[test]
fn rename_set_default_and_unregister_keep_repository_directories_untouched() {
    let sandbox = tempdir().unwrap();
    let work = sandbox.path().join("work");
    let personal = sandbox.path().join("personal");
    fs::create_dir(&work).unwrap();
    fs::create_dir(&personal).unwrap();
    let mut catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    catalog.register("work", &work).unwrap();
    let personal_entry = catalog.register("personal", &personal).unwrap();

    catalog.rename_registration("personal", "journal").unwrap();
    catalog.set_default("journal").unwrap();
    let removed = catalog.unregister("work").unwrap();

    assert_eq!(removed.name, "work");
    assert!(work.is_dir());
    assert_eq!(catalog.resolve_repo(None).unwrap().id, personal_entry.id);
    assert_eq!(
        catalog.resolve_repo(Some("journal")).unwrap().id,
        personal_entry.id
    );
}
