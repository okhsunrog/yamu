use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use yandex_music_api::{Client, auth::DeviceAuth};
use yandex_music_credentials::{CredentialStore, Credentials, DEFAULT_PROFILE, RefreshPolicy};

#[derive(Debug, Parser)]
#[command(about = "Manage shared Yandex Music credentials")]
struct Cli {
    /// Credential profile shared by workspace applications.
    #[arg(long, global = true, default_value = DEFAULT_PROFILE)]
    profile: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Authorize through Device Flow and persist the resulting credentials.
    Login {
        /// Replace an existing saved profile.
        #[arg(long)]
        force: bool,
    },
    /// Validate saved credentials against the account endpoint.
    Status,
    /// Force rotation of the stored access/refresh token pair.
    Refresh,
    /// Delete the saved credential profile.
    Logout,
    /// Print the credential file path without revealing its contents.
    Path,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = CredentialStore::open_default().context("failed to open credential store")?;

    match cli.command {
        Command::Login { force } => login(&store, &cli.profile, force).await,
        Command::Status => status(&store, &cli.profile).await,
        Command::Refresh => refresh(&store, &cli.profile).await,
        Command::Logout => logout(&store, &cli.profile),
        Command::Path => {
            println!("{}", store.profile_path(&cli.profile)?.display());
            Ok(())
        }
    }
}

async fn login(store: &CredentialStore, profile: &str, force: bool) -> Result<()> {
    if store.exists(profile)? && !force {
        bail!("profile {profile:?} already exists; pass --force to replace it");
    }

    let auth = DeviceAuth::new().context("failed to create Device Flow client")?;
    let token = auth
        .authorize(|code| {
            println!(
                "Откройте {} и введите код: {}",
                code.verification_url, code.user_code
            );
            println!("Код действителен {} секунд.", code.expires_in);
        })
        .await
        .context("Device Flow authorization failed")?;

    let client = Client::new(token.access_token())?;
    let account = client
        .account_status()
        .await
        .context("the token was issued but account validation failed")?;
    let credentials = Credentials::from_oauth_token(&token)?;
    let path = store.save(profile, &credentials)?;

    let name = account
        .account
        .as_ref()
        .and_then(|account| account.display_name.as_deref())
        .or_else(|| {
            account
                .account
                .as_ref()
                .and_then(|account| account.login.as_deref())
        })
        .unwrap_or("unknown account");
    println!("Авторизация успешна: {name}");
    println!("Credentials сохранены в {}", path.display());
    Ok(())
}

async fn status(store: &CredentialStore, profile: &str) -> Result<()> {
    let auth = DeviceAuth::new().context("failed to create OAuth client")?;
    let resolved = store
        .resolve(profile, &auth, RefreshPolicy::default())
        .await
        .with_context(|| format!("failed to load profile {profile:?}"))?;
    let credentials = &resolved.credentials;
    if credentials.is_expired()? {
        bail!("profile {profile:?} has expired; run `ym-auth login --force`");
    }

    let client = Client::new(credentials.access_token())?;
    let status = client
        .account_status()
        .await
        .context("saved credentials were rejected by Yandex Music")?;
    let account = status.account.as_ref();
    println!("profile: {profile}");
    println!("source: {}", resolved.source);
    println!("refreshed now: {}", resolved.refreshed);
    println!(
        "account: {}",
        account
            .and_then(|account| account.display_name.as_deref())
            .or_else(|| account.and_then(|account| account.login.as_deref()))
            .unwrap_or("unknown account")
    );
    match credentials.expires_in()? {
        Some(remaining) => println!("expires in: {} days", remaining.as_secs() / 86_400),
        None => println!("expires in: unknown"),
    }
    println!("refresh token: {}", credentials.refresh_token().is_some());
    Ok(())
}

async fn refresh(store: &CredentialStore, profile: &str) -> Result<()> {
    let auth = DeviceAuth::new().context("failed to create OAuth client")?;
    let (credentials, refreshed) = store
        .refresh_if_needed(profile, &auth, RefreshPolicy::force())
        .await
        .with_context(|| format!("failed to refresh profile {profile:?}"))?;
    let client = Client::new(credentials.access_token())?;
    client
        .account_status()
        .await
        .context("refreshed credentials were rejected by Yandex Music")?;
    println!("Profile {profile:?} refreshed: {refreshed}");
    println!(
        "expires in: {} days",
        credentials
            .expires_in()?
            .map_or(0, |remaining| remaining.as_secs() / 86_400)
    );
    Ok(())
}

fn logout(store: &CredentialStore, profile: &str) -> Result<()> {
    if store.delete(profile)? {
        println!("Удалён credential profile {profile:?}");
    } else {
        println!("Credential profile {profile:?} уже отсутствует");
    }
    Ok(())
}
