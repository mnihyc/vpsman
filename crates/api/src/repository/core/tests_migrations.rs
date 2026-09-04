use uuid::Uuid;

use crate::{model::CreateMigrationLinkRequest, routes_migrations::validate_create_migration_link};

#[test]
fn migration_link_validation_requires_confirmation() {
    let unconfirmed = CreateMigrationLinkRequest {
        restore_plan_id: Uuid::new_v4(),
        confirmed: false,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_migration_link(&unconfirmed)
            .unwrap_err()
            .code,
        "migration_confirmation_required"
    );

    let oversized_note = CreateMigrationLinkRequest {
        restore_plan_id: Uuid::new_v4(),
        confirmed: true,
        note: Some("x".repeat(1025)),
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_migration_link(&oversized_note)
            .unwrap_err()
            .code,
        "migration_note_too_long"
    );
}
