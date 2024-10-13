use axum::{
    extract::State,
    http::StatusCode,
    response::Html,
    routing::{get, post},
    Form, Router,
};
use clap::Parser;
use matrix_sdk::{
    config::SyncSettings,
    matrix_auth::{MatrixSession, MatrixSessionTokens},
    ruma::{
        api::client::{filter::FilterDefinition, sync::sync_events::v3::Filter},
        OwnedDeviceId, OwnedRoomAliasId, OwnedRoomId, OwnedUserId, UInt,
    },
    Client, SessionMeta,
};
use minijinja::{context, Environment};
use std::{collections::HashMap, sync::Arc, vec};

struct AppState {
    client: Client,
    rooms: Vec<RoomInfo>,
    env: Environment<'static>,
    turnstile_site_key: String,
    turnstile_secret_key: String,
}

#[derive(serde::Serialize)]
struct RoomInfo {
    room_id: OwnedRoomId,
    canonical_alias: Option<OwnedRoomAliasId>,
    name: Option<String>,
}

#[derive(serde::Deserialize)]
struct Invite {
    room_id: OwnedRoomId,
    user_id: OwnedUserId,
    #[serde(alias = "cf-turnstile-response")]
    cf_turnstile_response: String,
}

#[derive(serde::Deserialize)]
struct Turnstile {
    success: bool,
}

async fn index(State(state): State<Arc<AppState>>) -> Result<Html<String>, (StatusCode, String)> {
    Ok(Html(
        state
            .env
            .get_template("index.html")
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
            .render(context! {
                rooms => state.rooms,
                turnstile_site_key => state.turnstile_site_key,
            })
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?,
    ))
}

async fn invite(
    State(state): State<Arc<AppState>>,
    Form(invite): Form<Invite>,
) -> Result<Html<&'static str>, (StatusCode, String)> {
    let response: Turnstile = reqwest::Client::default()
        .post("https://challenges.cloudflare.com/turnstile/v0/siteverify")
        .form::<HashMap<String, String>>(
            &[
                ("secret".to_string(), state.turnstile_secret_key.clone()),
                ("response".to_string(), invite.cf_turnstile_response),
            ]
            .into(),
        )
        .send()
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
        .json()
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    if !response.success {
        return Err((
            StatusCode::FORBIDDEN,
            "invalid turnstile response".to_string(),
        ));
    }

    state
        .client
        .get_room(&invite.room_id)
        .ok_or((StatusCode::NOT_FOUND, "room not found".to_string()))?
        .invite_user_by_id(&invite.user_id)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Html("successfully invited"))
}

#[derive(clap::Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, env = "MATRIX_USER_ID")]
    user_id: OwnedUserId,
    #[arg(long, env = "MATRIX_DEVICE_ID")]
    device_id: OwnedDeviceId,
    #[arg(long, env = "MATRIX_ACCESS_TOKEN")]
    access_token: String,
    #[arg(long, env, default_value = "1x00000000000000000000AA")]
    turnstile_site_key: String,
    #[arg(long, env, default_value = "1x0000000000000000000000000000000AA")]
    turnstile_secret_key: String,
    #[arg(long)]
    listen_address: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let mut env = Environment::new();
    env.add_template("index.html", include_str!("../templates/index.html"))?;

    let Args {
        user_id,
        device_id,
        access_token,
        turnstile_site_key,
        turnstile_secret_key,
        listen_address,
    } = args;

    let client = Client::builder()
        .server_name(user_id.server_name())
        .build()
        .await?;

    client
        .matrix_auth()
        .restore_session(MatrixSession {
            meta: SessionMeta {
                user_id: user_id.clone(),
                device_id,
            },
            tokens: MatrixSessionTokens {
                access_token,
                refresh_token: None,
            },
        })
        .await?;

    let mut filter = FilterDefinition::ignore_all();
    filter.room.rooms = None;
    filter.room.timeline.limit = UInt::new(1);

    client
        .sync_once(SyncSettings::new().filter(Filter::FilterDefinition(filter)))
        .await?;

    let mut rooms = vec![];
    for room in client.rooms() {
        if room.can_user_invite(&user_id).await? {
            rooms.push(RoomInfo {
                room_id: room.room_id().to_owned(),
                canonical_alias: room.canonical_alias(),
                name: room.name(),
            });
        }
    }

    let state = Arc::new(AppState {
        client,
        rooms,
        env,
        turnstile_site_key,
        turnstile_secret_key,
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/invite", post(invite))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(listen_address).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
