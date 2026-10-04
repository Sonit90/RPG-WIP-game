use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::routes::{admin, auth, users};

#[derive(OpenApi)]
#[openapi(
    paths(
        auth::register,
        auth::login,
        auth::refresh,
        auth::logout,
        auth::verify_email_code,
        auth::resend_verification,
        auth::forgot_password,
        auth::reset_password,
        auth::change_password,
        auth::google_start,
        auth::google_callback,
        users::me,
        users::my_permissions,
        admin::list_users,
        admin::set_role,
        admin::set_active,
        admin::grant_permission,
        admin::delete_user,
    ),
    components(schemas(
        auth::RegisterReq,
        auth::TokenPair,
        auth::LoginReq,
        auth::RefreshReq,
        auth::LogoutReq,
        auth::OAuthCallbackQuery,
        auth::VerifyEmailCodeReq,
        auth::VerifyEmailCodeResp,
        auth::ResendVerificationReq,
        auth::ForgotPasswordReq,
        auth::ResetPasswordReq,
        auth::ChangePasswordReq,
        admin::Page,
        admin::SetRoleReq,
        admin::SetActiveReq,
        admin::GrantPermReq,
        generic_auth_core::models::User,
        generic_auth_core::models::Role,
    )),
    modifiers(&BearerAuth),
    info(
        title = "Generic Auth API",
        version = "0.1.0",
    ),
)]
pub struct ApiDoc;

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer_auth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .build(),
            ),
        );
    }
}
