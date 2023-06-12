use matrix_sdk::{
    config::SyncSettings,
    room::Room,
    ruma::{
        events::{
            reaction::SyncReactionEvent,
            relation::InReplyTo,
            room::{
                member::{MembershipState, SyncRoomMemberEvent},
                message::{Relation, RoomMessageEventContent},
                power_levels::RoomPowerLevelsEventContent,
            },
            StateEventType, TimelineEventType,
        },
        UserId,
    },
    Client,
};

use std::vec;

const REACTIONS: [&str; 7] = ["🎉", "🤣", "😃", "😋", "🥳", "🤔", "😅"];

fn hash_user_id(user_id: &UserId) -> &str {
    let hash = xxhash_rust::xxh3::xxh3_64(user_id.as_bytes());
    REACTIONS[hash as usize % REACTIONS.len()]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let username = std::env::var("BOUNCER_USERNAME")?;
    let password = std::env::var("BOUNCER_PASSWORD")?;

    let username = UserId::parse(username)?;

    let client = Client::builder()
        .server_name(username.server_name())
        .build()
        .await?;

    log::info!("logging in as user {}", username);

    client.login_username(username, &password).send().await?;

    let init = client.sync_once(SyncSettings::default()).await?;

    log::info!("initial sync complete");

    let rooms = client.joined_rooms();

    for room in rooms {
        log::info!("protecting room {} ({:?})", room.room_id(), room.name());
        let power_levels = room
            .get_state_event_static::<RoomPowerLevelsEventContent>()
            .await?
            .unwrap()
            .deserialize()?
            .power_levels();
        if power_levels.users_default >= power_levels.events_default {
            log::warn!(
                "users in room {} ({:?}) can send message by default",
                room.room_id(),
                room.name()
            );
        }
        if power_levels.users_default
            < *power_levels
                .events
                .get(&TimelineEventType::Reaction)
                .unwrap()
        {
            log::warn!(
                "users in room {} ({:?}) cannot send reaction by default",
                room.room_id(),
                room.name()
            );
        }
        if !power_levels.user_can_send_state(room.own_user_id(), StateEventType::RoomPowerLevels) {
            log::warn!(
                "bouncer in room {} ({:?}) cannot change user power levels",
                room.room_id(),
                room.name()
            );
        }
    }

    client.add_event_handler(|event: SyncRoomMemberEvent, room: Room| async move {
        if let Room::Joined(room) = room {
            if event.membership() == &MembershipState::Join {
                let ts = event
                    .origin_server_ts()
                    .to_system_time()
                    .ok_or(anyhow::anyhow!("origin server ts cannot be represented"))?;
                if std::time::SystemTime::now().duration_since(ts)?.as_secs() > 600 {
                    log::warn!(
                        "event {} older than 600 seconds, ignoring",
                        event.event_id()
                    );
                    return Ok(());
                }
                log::info!(
                    "user {} joined {} ({:?})",
                    event.sender(),
                    room.room_id(),
                    room.name(),
                );
                let mut content = RoomMessageEventContent::notice_plain(format!(
                    "新加群的用户 {} 请用 Reaction {} 回复本条消息",
                    event.sender(),
                    hash_user_id(event.sender())
                ));
                content.relates_to = Some(Relation::Reply {
                    in_reply_to: InReplyTo::new(event.event_id().into()),
                });
                room.send(content, None).await?;
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    client.add_event_handler(|event: SyncReactionEvent, room: Room| async move {
        if let Room::Joined(room) = room {
            if let Some(event) = event.as_original() {
                if event.content.relates_to.key == hash_user_id(&event.sender)
                    && room
                        .event(&event.content.relates_to.event_id)
                        .await?
                        .event
                        .deserialize()?
                        .sender()
                        == room.own_user_id()
                {
                    log::info!("user {} passed verification", event.sender);
                    let power_levels = room
                        .get_state_event_static::<RoomPowerLevelsEventContent>()
                        .await?
                        .unwrap()
                        .deserialize()?
                        .power_levels();
                    room.update_power_levels(vec![(&event.sender, power_levels.event_default)])
                        .await?;
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    client
        .sync(SyncSettings::default().token(init.next_batch))
        .await?;

    Ok(())
}
