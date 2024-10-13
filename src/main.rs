use axum::{
    extract::State,
    http::StatusCode,
    response::Html,
    routing::{get, post},
    Form, Router,
};
use matrix_sdk::{
    config::SyncSettings,
    matrix_auth::{MatrixSession, MatrixSessionTokens},
    ruma::{
        api::client::{filter::FilterDefinition, sync::sync_events::v3::Filter},
        OwnedDeviceId, OwnedRoomAliasId, OwnedRoomId, OwnedUserId, UInt, UserId,
    },
    Client, SessionMeta,
};
use minijinja::{context, Environment};
use std::{sync::Arc, vec};

struct AppState {
    client: Client,
    rooms: Vec<RoomInfo>,
    env: Environment<'static>,
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
}

async fn index(State(state): State<Arc<AppState>>) -> Result<Html<String>, (StatusCode, String)> {
    Ok(Html(
        state
            .env
            .get_template("index.html")
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
            .render(context! {
                rooms => state.rooms,
            })
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?,
    ))
}

async fn invite(
    State(state): State<Arc<AppState>>,
    Form(invite): Form<Invite>,
) -> Result<Html<&'static str>, (StatusCode, String)> {
    state
        .client
        .get_room(&invite.room_id)
        .ok_or((StatusCode::NOT_FOUND, "room not found".to_string()))?
        .invite_user_by_id(&invite.user_id)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Html("successfully invited"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut env = Environment::new();
    env.add_template("index.html", include_str!("../templates/index.html"))?;

    let user_id = UserId::parse(std::env::var("MATRIX_USER_ID")?)?;
    let device_id: OwnedDeviceId = std::env::var("MATRIX_DEVICE_ID")?.into();
    let access_token = std::env::var("MATRIX_ACCESS_TOKEN")?;
    let listen_address = std::env::var("LISTEN_ADDRESS")?;

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

    let state = Arc::new(AppState { client, rooms, env });

    let app = Router::new()
        .route("/", get(index))
        .route("/invite", post(invite))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(listen_address).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
