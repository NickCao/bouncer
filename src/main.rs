use axum::{
    extract::State,
    http::StatusCode,
    response::Html,
    routing::{get, post},
    Form, Router,
};
use clap::Parser;
use minijinja::{context, Environment};
use ruma::{space::SpaceRoomJoinRule, Client, OwnedRoomAliasId, OwnedRoomId, OwnedUserId};
use std::{collections::HashMap, sync::Arc};

struct AppState {
    client: Client<ruma::client::http_client::Reqwest>,
    rooms: HashMap<OwnedRoomId, RoomInfo>,
    env: Environment<'static>,
    turnstile_site_key: String,
    turnstile_secret_key: String,
}

#[derive(serde::Serialize)]
struct RoomInfo {
    room_id: OwnedRoomId,
    canonical_alias: Option<OwnedRoomAliasId>,
    name: Option<String>,
    join_rule: SpaceRoomJoinRule,
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

async fn index(State(state): State<Arc<AppState>>) -> Result<Html<String>, StatusCode> {
    Ok(Html(
        state
            .env
            .get_template("index.html")
            .map_err(|err| {
                log::error!("failed to get template: {}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .render(context! {
                rooms => state.rooms.values().collect::<Vec<_>>(),
                turnstile_site_key => state.turnstile_site_key,
            })
            .map_err(|err| {
                log::error!("failed to render template: {}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

async fn invite(
    State(state): State<Arc<AppState>>,
    Form(invite): Form<Invite>,
) -> Result<String, (StatusCode, String)> {
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
        .map_err(|err| {
            log::error!("failed to verify turnstile response: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to verify turnstile response".to_string(),
            )
        })?
        .json()
        .await
        .map_err(|err| {
            log::error!("failed to decode turnstile verify result: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to decode turnstile verify result".to_string(),
            )
        })?;

    if !response.success {
        return Err((
            StatusCode::FORBIDDEN,
            "turnstile verification failed".to_string(),
        ));
    }

    if !state.rooms.contains_key(&invite.room_id) {
        return Err((StatusCode::BAD_REQUEST, "invalid room_id".to_string()));
    }

    let profile = state
        .client
        .send_request(ruma::api::client::profile::get_profile::v3::Request::new(
            invite.user_id.clone(),
        ))
        .await
        .map_err(|err| {
            log::error!(
                "failed to get user profile for {}: {}",
                &invite.user_id,
                err
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get user profile".to_string(),
            )
        })?;

    state
        .client
        .send_request(
            ruma::api::client::membership::invite_user::v3::Request::new(
                invite.room_id.clone(),
                ruma::api::client::membership::invite_user::v3::InvitationRecipient::UserId {
                    user_id: invite.user_id.clone(),
                },
            ),
        )
        .await
        .map_err(|err| {
            log::error!(
                "failed to invite user {} to room {}: {}",
                &invite.user_id,
                &invite.room_id,
                err
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get invite user".to_string(),
            )
        })?;

    Ok(format!(
        "successfully invited user {} ({}) to room {}",
        profile.displayname.unwrap_or_default(),
        invite.user_id,
        invite.room_id,
    ))
}

#[derive(clap::Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, env = "MATRIX_ACCESS_TOKEN")]
    access_token: String,
    #[arg(long, env = "MATRIX_HOMESERVER_URL")]
    homeserver_url: String,
    #[arg(long, env, default_value = "1x00000000000000000000AA")]
    turnstile_site_key: String,
    #[arg(long, env, default_value = "1x0000000000000000000000000000000AA")]
    turnstile_secret_key: String,
    #[arg(long)]
    listen_address: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args = Args::parse();

    let mut env = Environment::new();
    env.add_template("index.html", include_str!("../templates/index.html"))?;

    let Args {
        access_token,
        homeserver_url,
        turnstile_site_key,
        turnstile_secret_key,
        listen_address,
    } = args;

    let client = Client::builder()
        .homeserver_url(homeserver_url)
        .access_token(Some(access_token))
        .build::<ruma::client::http_client::Reqwest>()
        .await
        .unwrap();

    let joined_rooms = client
        .send_request(ruma::api::client::membership::joined_rooms::v3::Request::new())
        .await?
        .joined_rooms;

    let mut rooms = HashMap::default();
    for room in joined_rooms {
        let preview = client
            .send_request(ruma::api::client::room::get_summary::msc3266::Request::new(
                room.into(),
                vec![],
            ))
            .await?;
        rooms.insert(
            preview.room_id.clone(),
            RoomInfo {
                room_id: preview.room_id,
                canonical_alias: preview.canonical_alias,
                name: preview.name,
                join_rule: preview.join_rule,
            },
        );
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
