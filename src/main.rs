use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
    Form, Router,
};
use bouncer::{AppState, Invite, RoomInfo};
use chrono::{Duration, Local};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use clap::Parser;
use oauth2::{
    basic::BasicClient, reqwest::async_http_client, AuthUrl, AuthorizationCode, ClientId,
    ClientSecret, CsrfToken, RedirectUrl, TokenResponse, TokenUrl,
};
use ruma::{
    api::client,
    events::{
        room::power_levels::{RoomPowerLevels, RoomPowerLevelsEventContent},
        StateEventType,
    },
    Client,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

#[derive(serde::Deserialize)]
struct Turnstile {
    success: bool,
}

#[derive(Debug, serde::Deserialize)]
struct Callback {
    code: String,
    state: String,
}

#[derive(Debug, serde::Deserialize)]
struct GitHubUser {
    login: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn callback(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Callback>,
) -> Result<String, (StatusCode, String)> {
    let invite = state
        .csrf
        .lock()
        .await
        .remove(&query.state)
        .ok_or((StatusCode::BAD_REQUEST, "invalid csrf token".to_string()))?;

    let token = state
        .oauth2_client
        .exchange_code(AuthorizationCode::new(query.code))
        .request_async(async_http_client)
        .await
        .map_err(|err| {
            log::error!("failed to exchange for token: {}", err);
            (
                StatusCode::BAD_REQUEST,
                "failed to exchange for token".to_string(),
            )
        })?;

    let user: GitHubUser = reqwest::Client::builder()
        .user_agent("Matrix Bouncer")
        .build()
        .map_err(|err| {
            log::error!("failed to build client: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to build client".to_string(),
            )
        })?
        .get("https://api.github.com/user")
        .bearer_auth(token.access_token().secret())
        .send()
        .await
        .map_err(|err| {
            log::error!("failed to get user info: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get user info".to_string(),
            )
        })?
        .json()
        .await
        .map_err(|err| {
            log::error!("failed to decode user info: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to decode user info".to_string(),
            )
        })?;

    let age = Local::now().to_utc().signed_duration_since(user.created_at);

    log::warn!(
        "matrix user {} is GitHub user {}, age {:?}",
        &invite.user_id,
        &user.login,
        HumanTime::from(age).to_text_en(Accuracy::Rough, Tense::Present),
    );

    if invite.user_id.server_name() == "matrix.org" && age.le(&Duration::days(1)) {
        log::error!(
            "matrix user {} is from matrix.org and GitHub user {} age {:?} less than 1 day",
            &invite.user_id,
            &user.login,
            HumanTime::from(age).to_text_en(Accuracy::Rough, Tense::Present),
        );
        return Err((StatusCode::FORBIDDEN, "".to_string()));
    }

    let profile = state
        .client
        .send_request(client::profile::get_profile::v3::Request::new(
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
        .send_request(client::membership::invite_user::v3::Request::new(
            invite.room_id.clone(),
            client::membership::invite_user::v3::InvitationRecipient::UserId {
                user_id: invite.user_id.clone(),
            },
        ))
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
                "failed to invite user".to_string(),
            )
        })?;

    Ok(format!(
        "successfully invited user {} ({}) to room {}",
        profile.displayname.unwrap_or_default(),
        invite.user_id,
        invite.room_id,
    ))
}

async fn invite(
    State(state): State<Arc<AppState>>,
    Form(invite): Form<Invite>,
) -> Result<Redirect, (StatusCode, String)> {
    let response: Turnstile = reqwest::Client::new()
        .post("https://challenges.cloudflare.com/turnstile/v0/siteverify")
        .form::<HashMap<String, String>>(
            &[
                ("secret".to_string(), state.turnstile_secret_key.clone()),
                ("response".to_string(), invite.cf_turnstile_response.clone()),
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

    let (auth_url, csrf_token) = state
        .oauth2_client
        .authorize_url(CsrfToken::new_random)
        .url();

    state
        .csrf
        .lock()
        .await
        .insert(csrf_token.secret().to_string(), invite);

    Ok(Redirect::to(auth_url.as_str()))
}

#[derive(clap::Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, env = "MATRIX_ACCESS_TOKEN")]
    access_token: String,
    #[arg(long, env = "MATRIX_HOMESERVER_URL")]
    homeserver_url: String,
    #[arg(long, env = "GITHUB_CLIENT_ID")]
    github_client_id: String,
    #[arg(long, env = "GITHUB_CLIENT_SECRET")]
    github_client_secret: String,
    #[arg(long, env = "GITHUB_REDIRECT_URL")]
    github_redirect_url: String,
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

    let Args {
        access_token,
        homeserver_url,
        github_client_id,
        github_client_secret,
        github_redirect_url,
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

    let user_id = client
        .send_request(client::account::whoami::v3::Request::new())
        .await?
        .user_id;
    log::warn!("Running under user {}", &user_id);

    let joined_rooms = client
        .send_request(client::membership::joined_rooms::v3::Request::new())
        .await?
        .joined_rooms;

    let mut rooms = HashMap::default();
    for room_id in joined_rooms {
        let power_levels: RoomPowerLevels = client
            .send_request(client::state::get_state_events_for_key::v3::Request::new(
                room_id.clone(),
                StateEventType::RoomPowerLevels,
                "".to_string(),
            ))
            .await?
            .content
            .deserialize_as::<RoomPowerLevelsEventContent>()?
            .into();
        if !power_levels.user_can_invite(&user_id) {
            log::warn!(
                "Do not have invite permission for room {}, ignoring",
                &room_id
            );
            continue;
        };
        let preview = client
            .send_request(client::room::get_summary::msc3266::Request::new(
                room_id.clone().into(),
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

    let oauth2_client = BasicClient::new(
        ClientId::new(github_client_id),
        Some(ClientSecret::new(github_client_secret)),
        AuthUrl::new("https://github.com/login/oauth/authorize".to_string())?,
        Some(TokenUrl::new(
            "https://github.com/login/oauth/access_token".to_string(),
        )?),
    )
    .set_redirect_uri(RedirectUrl::new(github_redirect_url)?);

    let state = Arc::new(AppState {
        client,
        oauth2_client,
        rooms,
        turnstile_site_key,
        turnstile_secret_key,
        csrf: Mutex::new(HashMap::new()),
    });

    let app = Router::new()
        .route("/", get(bouncer::index))
        .route("/invite", post(invite))
        .route("/callback", get(callback))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(listen_address).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
