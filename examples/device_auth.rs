use yamu::{Client, Result, auth::DeviceAuth};

#[tokio::main]
async fn main() -> Result<()> {
    let auth = DeviceAuth::new()?;
    let token = auth
        .authorize(|code| {
            println!(
                "Откройте {} и введите код: {}",
                code.verification_url, code.user_code
            );
            println!("Код действителен {} секунд.", code.expires_in);
        })
        .await?;

    let client = Client::new(token.access_token())?;
    let status = client.account_status().await?;
    let account = status.account.as_ref();
    let name = account
        .and_then(|account| account.display_name.as_deref())
        .or_else(|| account.and_then(|account| account.login.as_deref()))
        .unwrap_or("пользователь");
    println!("Авторизация успешна: {name}");

    let search = client.search("Boards of Canada").await?;
    let found = search.tracks.map_or(0, |tracks| tracks.results.len());
    println!("Поиск работает, треков на первой странице: {found}");

    let tracks = client.tracks(["10994777:1193829"]).await?;
    println!("Получено треков по ID: {}", tracks.len());

    let album = client.album_with_tracks(1_193_829_u64).await?;
    println!(
        "Альбом: {}",
        album.title.as_deref().unwrap_or("без названия")
    );

    Ok(())
}
