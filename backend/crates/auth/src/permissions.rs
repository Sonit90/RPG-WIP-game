use std::collections::HashSet;

use generic_auth_core::AppError;
use generic_auth_db::users as user_db;
use sqlx::PgPool;
use uuid::Uuid;

/// Strongly-typed permission identifiers. Keep in sync with the migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    UsersRead,
    UsersManage,
    UsersAssignRoles,
    FilesUpload,
    FilesReadOwn,
    FilesReadAll,
    FilesDeleteOwn,
    FilesDeleteAny,
    FormatsManageOwn,
    FormatsManageAny,
    MappingsManageOwn,
    MappingsManageAny,
    JobsRun,
}

impl Permission {
    pub fn as_str(&self) -> &'static str {
        match self {
            Permission::UsersRead => "users.read",
            Permission::UsersManage => "users.manage",
            Permission::UsersAssignRoles => "users.assign_roles",
            Permission::FilesUpload => "files.upload",
            Permission::FilesReadOwn => "files.read_own",
            Permission::FilesReadAll => "files.read_all",
            Permission::FilesDeleteOwn => "files.delete_own",
            Permission::FilesDeleteAny => "files.delete_any",
            Permission::FormatsManageOwn => "formats.manage_own",
            Permission::FormatsManageAny => "formats.manage_any",
            Permission::MappingsManageOwn => "mappings.manage_own",
            Permission::MappingsManageAny => "mappings.manage_any",
            Permission::JobsRun => "jobs.run",
        }
    }
}

pub async fn load_permissions(pool: &PgPool, user_id: Uuid) -> Result<HashSet<String>, AppError> {
    let v = user_db::permissions_for(pool, user_id).await?;
    Ok(v.into_iter().collect())
}

pub fn require(perms: &HashSet<String>, p: Permission) -> Result<(), AppError> {
    if perms.contains(p.as_str()) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}
