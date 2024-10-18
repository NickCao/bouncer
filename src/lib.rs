use std::{collections::HashMap, sync::Arc};

use axum::extract::State;
use maud::{html, Markup, DOCTYPE};
use oauth2::basic::BasicClient;
use ruma::{space::SpaceRoomJoinRule, Client, OwnedRoomAliasId, OwnedRoomId, OwnedUserId};
use tokio::sync::Mutex;

pub struct AppState {
    pub client: Client<ruma::client::http_client::Reqwest>,
    pub oauth2_client: BasicClient,
    pub rooms: HashMap<OwnedRoomId, RoomInfo>,
    pub turnstile_site_key: String,
    pub turnstile_secret_key: String,
    pub csrf: Mutex<HashMap<String, Invite>>,
}

#[derive(serde::Serialize)]
pub struct RoomInfo {
    pub room_id: OwnedRoomId,
    pub canonical_alias: Option<OwnedRoomAliasId>,
    pub name: Option<String>,
    pub join_rule: SpaceRoomJoinRule,
}

#[derive(serde::Deserialize)]
pub struct Invite {
    pub room_id: OwnedRoomId,
    pub user_id: OwnedUserId,
    #[serde(alias = "cf-turnstile-response")]
    pub cf_turnstile_response: String,
}

pub async fn index(State(state): State<Arc<AppState>>) -> Markup {
    let rooms = state.rooms.values().collect::<Vec<_>>();
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Matrix Bouncer" }
                script src="https://challenges.cloudflare.com/turnstile/v0/api.js" async defer {}
                style {
                    r#"
                      table, th, td {
                        border: 1px solid;
                      }
                      th, td {
                        padding: 5px;
                      }
                      table {
                        border-collapse: collapse;
                      }
                    "#
                }
            }
            body {
                div {
                    form action="invite" method="post" {
                        table {
                            thead {
                                tr {
                                    th { "Select" }
                                    th { "Name" }
                                    th { "Alias" }
                                    th { "Join Rule" }
                                    th { "ID" }
                                }
                            }
                            tbody {
                                @for room in &rooms {
                                    tr {
                                        td {
                                            input type="radio" name="room_id" value=(room.room_id);
                                        }
                                        td { (room.name.clone().unwrap_or_default()) }
                                        td {
                                          (room.canonical_alias
                                            .as_ref()
                                            .map(OwnedRoomAliasId::to_string)
                                            .unwrap_or_default())
                                        }
                                        td { (room.join_rule) }
                                        td { (room.room_id) }
                                    }
                                }
                            }
                        }
                        div style="display: flex; padding: 5px;" {
                          div style="display: flex; flex-direction: column;" {
                            div style="padding: 5px;" {
                                label for="user" style="padding-right: 5px;" { "User ID" }
                                input type="text" id="user" name="user_id" placeholder="@user:example.com" required;
                            }
                            div style="padding: 5px;" {
                              button type="submit" style="width: 100%;" { "Login with GitHub to Invite" }
                            }
                          }
                          div class="cf-turnstile" data-sitekey=(&state.turnstile_site_key) style="padding: 5px;" {}
                        }
                    }
                }
                footer {
                  "Source Code:" a href="https://github.com/NickCao/bouncer" { "https://github.com/NickCao/bouncer" }
                }
            }
        }
    }
}
