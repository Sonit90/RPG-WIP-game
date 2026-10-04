use std::sync::Arc;

use clap::{Parser, Subcommand};
use generic_auth_api::{
    commands::create_admin::{create_admin_user, CreateAdminArgs},
    config::Settings,
    run,
};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[derive(Parser)]
#[command(name = "generic-auth-api", about = "Generic RBAC auth API server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create an admin user directly in the database
    CreateAdmin {
        #[arg(long)]
        email: String,
        #[arg(long)]
        password: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    init_tracing();

    let cli = Cli::parse();
    let settings = Settings::load()?;

    match cli.command {
        None => {
            tracing::info!(env = %settings.app_env, "starting generic-auth-api");
            run(Arc::new(settings)).await
        }
        Some(Command::CreateAdmin { email, password }) => {
            let db = generic_auth_db::connect(&generic_auth_db::DbConfig {
                url: settings.db.url.clone(),
                max_connections: 2,
                min_connections: 1,
                acquire_timeout_secs: settings.db.acquire_timeout_secs,
            }).await?;

            let user = create_admin_user(&db, CreateAdminArgs { email, password })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            println!("Admin created: {} ({})", user.email.unwrap_or_default(), user.id);
            Ok(())
        }
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,generic_auth=debug,sqlx=warn"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true).compact())
        .init();
}
