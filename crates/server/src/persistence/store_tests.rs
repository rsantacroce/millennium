//! Store tests shared by SQLite and Postgres: both stores expand `store_tests!` inside their own
//! test module (so private methods are reachable), giving each the same behaviour checks.
//!
//! The caller defines `async fn store() -> Option<Store>`: `None` skips the test (Postgres runs
//! only when `TEST_DATABASE_URL` points at a server, as in CI).

macro_rules! store_tests {
    ($store:ty) => {
    const LIMITS: HistoryLimits = HistoryLimits { global: 50, whispers: 50 };

    async fn account(store: &mut $store, name: &str) -> i64 {
        store.create_account(name, "hash").await.unwrap().unwrap().account_id
    }

    async fn character(store: &mut $store, account_id: i64, name: &str) -> Result<i64, String> {
        let new = NewCharacter {
            name: name.into(),
            appearance: Appearance::default(),
            zone: ZoneId(1),
            position: Vec2::new(1.0, 2.0),
            state: PlayerState::new(25),
        };
        store.create_character(account_id, &new).await.unwrap()?;
        match store.list(Entity::PLACEHOLDER, account_id).await.unwrap() {
            DbResult::CharacterList { characters, .. } => {
                Ok(characters.into_iter().find(|c| c.name == name).unwrap().id as i64)
            }
            _ => unreachable!(),
        }
    }

    async fn load(store: &mut $store, account_id: i64, id: i64) -> LoginData {
        store.load(account_id, id, LIMITS).await.unwrap().expect("character exists")
    }

    #[tokio::test]
    async fn accounts_are_unique_and_case_insensitive() {
        let Some(mut s) = store().await else { return };
        account(&mut s, "Neo").await;
        assert!(s.create_account("NEO", "x").await.unwrap().is_none());
        let creds = s.credentials("neo").await.unwrap().unwrap();
        assert_eq!(creds.username, "Neo");
        assert!(s.credentials("trinity").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn character_limits_and_ownership() {
        let Some(mut s) = store().await else { return };
        let neo = account(&mut s, "Neo").await;
        let smith = account(&mut s, "Smith").await;
        let id = character(&mut s, neo, "Neo").await.unwrap();
        character(&mut s, neo, "Thomas").await.unwrap();
        // Names are unique across accounts, case-insensitively.
        assert!(character(&mut s, smith, "THOMAS").await.unwrap_err().contains("taken"));
        character(&mut s, neo, "Anderson").await.unwrap();
        assert!(character(&mut s, neo, "TheOne").await.unwrap_err().contains("at most 3"));
        // Smith can't load Neo's character.
        assert!(s.load(smith, id, LIMITS).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn state_and_position_roundtrip() {
        let content = shared::content::Content::builtin();
        let Some(mut s) = store().await else { return };
        let acc = account(&mut s, "Neo").await;
        let id = character(&mut s, acc, "Neo").await.unwrap();
        let fresh = load(&mut s, acc, id).await.character;
        assert_eq!(fresh.position, Vec2::new(1.0, 2.0));
        assert_eq!(fresh.state, PlayerState::new(25));

        let mut state = fresh.state.clone();
        state
            .apply(
                &[
                    Effect::StartQuest("56k_dreams".into()),
                    Effect::GiveItem("soda".into(), 12),
                    Effect::GiveItem("pager".into(), 1),
                    Effect::TakeCash(5),
                ],
                &content,
            )
            .unwrap();
        state.skills.insert(Skill::Electronics, 123);
        state.weapon = Some("baseball_bat".into());
        state.kills.insert("dock_rat".into(), 4);
        state.gear.insert(shared::content::GearSlot::Body, "windbreaker".into());
        state.rep.insert("meridian".into(), 150);
        state.rep.insert("dockside".into(), -20);
        // Buffs and cooldowns survive; ones already over are dropped.
        let now = now_ms();
        state.buffs.push(shared::game::ActiveBuff { source: "eggs".into(), skill: Skill::Cooking, levels: 2, until_ms: now + 600_000 });
        state.start_cooldown("hack:payphone", now + 300_000, now);
        let mut saved = state.clone();
        saved.buffs.push(shared::game::ActiveBuff { source: "soda".into(), skill: Skill::Fitness, levels: 1, until_ms: now - 1 });
        saved.cooldowns.insert("hack:atm".into(), now - 1);
        s.handle(DbCommand::SaveState { character_id: id, state: saved }, LIMITS).await;
        s.handle(DbCommand::SaveState { character_id: id, state: state.clone() }, LIMITS).await;
        s.handle(
            DbCommand::SavePositions(vec![SavedPosition { character_id: id, zone: ZoneId(1), position: Vec2::new(5.0, -6.0) }]),
            LIMITS,
        )
        .await;

        let loaded = load(&mut s, acc, id).await.character;
        assert_eq!(loaded.state, state);
        assert_eq!(loaded.position, Vec2::new(5.0, -6.0));
    }

    #[tokio::test]
    async fn history_and_ignores() {
        let Some(mut s) = store().await else { return };
        let a = account(&mut s, "Alice").await;
        let b = account(&mut s, "Bob").await;
        let c = account(&mut s, "Carol").await;
        let alice = character(&mut s, a, "Alice").await.unwrap();
        let bob = character(&mut s, b, "Bob").await.unwrap();
        let carol = character(&mut s, c, "Carol").await.unwrap();
        for (channel, sender, recipient, t) in [
            (ChatChannel::Global, bob, None, 1),
            (ChatChannel::Whisper, bob, Some(alice), 2),
            (ChatChannel::Whisper, bob, Some(carol), 3), // not Alice's business
            (ChatChannel::Say, bob, None, 4),            // say isn't replayed
            (ChatChannel::Global, carol, None, 5),
        ] {
            let record = ChatRecord { channel, sender_id: sender, recipient_id: recipient, zone: None, body: format!("msg{t}"), sent_at_ms: t };
            s.handle(DbCommand::InsertChat(record), LIMITS).await;
        }
        let texts = |d: &LoginData| d.history.iter().map(|l| l.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&load(&mut s, a, alice).await), ["msg1", "msg2", "msg5"]);

        s.set_ignore(alice, "carol", true).await.unwrap();
        let data = load(&mut s, a, alice).await;
        assert_eq!(data.ignores, vec![(carol as u64, "Carol".to_string())]);
        assert_eq!(texts(&data), ["msg1", "msg2"]);
    }

    #[tokio::test]
    async fn friends_and_crews() {
        let Some(mut s) = store().await else { return };
        let a = account(&mut s, "Alice").await;
        let b = account(&mut s, "Bob").await;
        let alice = character(&mut s, a, "Alice").await.unwrap();
        let bob = character(&mut s, b, "Bob").await.unwrap();

        assert_eq!(s.set_friend(alice, "bob", true).await.unwrap(), Some((bob as u64, "Bob".into())));
        assert_eq!(s.set_friend(alice, "nobody", true).await.unwrap(), None);
        assert_eq!(load(&mut s, a, alice).await.friends, vec![(bob as u64, "Bob".to_string())]);

        let crew = s.create_crew(alice, "Wharf Rats", "WRC").await.unwrap().unwrap();
        assert!(s.create_crew(bob, "wharf rats", "XYZ").await.unwrap().is_err(), "names are unique");
        assert!(s.create_crew(alice, "Other", "OTH").await.unwrap().is_err(), "one crew at a time");
        s.handle(DbCommand::JoinCrew { link: Entity::PLACEHOLDER, character_id: bob, crew_id: crew }, LIMITS).await;
        let roster = s.load_crew(crew).await.unwrap().unwrap();
        assert_eq!(roster.tag, "WRC");
        assert_eq!(roster.members[0], (alice, "Alice".to_string(), CrewRank::Leader));
        assert_eq!(roster.members[1], (bob, "Bob".to_string(), CrewRank::Member));
        assert_eq!(load(&mut s, b, bob).await.crew, Some(crew));

        s.handle(DbCommand::DisbandCrew { link: Entity::PLACEHOLDER, crew_id: crew }, LIMITS).await;
        assert!(s.load_crew(crew).await.unwrap().is_none());
        assert_eq!(load(&mut s, b, bob).await.crew, None);
    }

    #[tokio::test]
    async fn marketplace_auctions() {
        let Some(mut s) = store().await else { return };
        let mut ids = Vec::new();
        for name in ["Alice", "Bob", "Carol"] {
            let a = account(&mut s, name).await;
            ids.push((a, character(&mut s, a, name).await.unwrap()));
        }
        let [(_, alice), (b_acc, bob), (_, carol)] = ids[..] else { unreachable!() };
        let st = PlayerState::new(100);
        let walkman = NewListing {
            stack: ItemStack { item: "walkman".into(), qty: 1 },
            item_name: "Walkman".into(),
            start_price: 10,
            buyout: Some(50),
            expires_at_ms: now_ms() + 60_000,
            max_active: 10,
        };
        assert!(s.market_create(alice, &st, &walkman).await.unwrap().is_ok());
        let pager = NewListing { item_name: "Pager".into(), buyout: None, ..walkman.clone() };
        assert!(s.market_create(alice, &st, &pager).await.unwrap().is_ok());

        let (found, total) = s.market_search("walk", 0).await.unwrap();
        assert_eq!((found.len(), total), (1, 1));
        assert_eq!(found[0].seller, "Alice");
        let w = found[0].id;
        assert_eq!(s.market_search("", 0).await.unwrap().1, 2);
        let p = s.market_search("PAGER", 0).await.unwrap().0[0].id;

        // Bidding rules.
        assert!(s.market_bid(alice, &st, w, 10).await.unwrap().0.is_err(), "own auction");
        assert!(s.market_bid(bob, &st, w, 9).await.unwrap().0.is_err(), "below start");
        assert!(s.market_bid(bob, &st, w, 50).await.unwrap().0.is_err(), "that's a buyout");
        assert!(s.market_bid(bob, &st, w, 10).await.unwrap().0.is_ok());
        assert!(s.market_bid(bob, &st, w, 20).await.unwrap().0.is_err(), "already high bidder");
        assert!(s.market_bid(carol, &st, w, 10).await.unwrap().0.is_err(), "must raise");
        let (ok, notify) = s.market_bid(carol, &st, w, 11).await.unwrap();
        assert!(ok.is_ok());
        assert_eq!(notify[0].0, bob);
        let bob_claims = s.market_claims(bob).await.unwrap();
        assert_eq!((bob_claims[0].cash, bob_claims[0].item.clone()), (10, None), "Bob's bid is refunded");

        // Bob buys it outright; Carol is refunded, Alice gets paid minus the cut.
        let (ok, notify) = s.market_buyout(bob, &st, w, 50).await.unwrap();
        assert!(ok.is_ok(), "{ok:?}");
        assert_eq!(notify.len(), 2);
        assert_eq!(s.market_claims(carol).await.unwrap()[0].cash, 11);
        assert_eq!(s.market_claims(alice).await.unwrap()[0].cash, 48);
        let bob_claims = s.market_claims(bob).await.unwrap();
        assert_eq!(bob_claims[1].item, Some(ItemStack { item: "walkman".into(), qty: 1 }));
        assert!(s.market_bid(carol, &st, w, 60).await.unwrap().0.is_err(), "sold");
        assert_eq!(s.market_mine(alice).await.unwrap().len(), 1);

        // Collecting saves the state and removes the claims, exactly once.
        let ids: Vec<i64> = bob_claims.iter().map(|c| c.id).collect();
        let mut rich = PlayerState::new(160);
        rich.inventory[0] = Some(ItemStack { item: "walkman".into(), qty: 1 });
        assert!(s.market_collect(bob, &ids, &rich).await.unwrap().is_ok());
        assert!(s.market_collect(bob, &ids, &rich).await.unwrap().is_err());
        assert!(s.market_claims(bob).await.unwrap().is_empty());
        assert_eq!(load(&mut s, b_acc, bob).await.character.state.cash, 160);

        // The pager: cancelled with no bids; then an expiring one goes to the high bidder.
        assert!(s.market_cancel(bob, p).await.unwrap().is_err(), "not Bob's");
        assert!(s.market_cancel(alice, p).await.unwrap().is_ok());
        assert_eq!(s.market_claims(alice).await.unwrap().len(), 2);
        let soon = NewListing { expires_at_ms: now_ms() + 1_000, ..pager.clone() };
        assert!(s.market_create(alice, &st, &soon).await.unwrap().is_ok());
        let unsold = NewListing { expires_at_ms: now_ms() + 1_000, item_name: "Mixtape".into(), ..pager };
        assert!(s.market_create(alice, &st, &unsold).await.unwrap().is_ok());
        let soon_id = s.market_search("pager", 0).await.unwrap().0[0].id;
        assert!(s.market_bid(carol, &st, soon_id, 12).await.unwrap().0.is_ok());
        assert!(s.market_cancel(alice, soon_id).await.unwrap().is_err(), "has bids");
        let notify = s.market_expire(now_ms() + 2_000).await.unwrap();
        assert_eq!(notify.len(), 3, "{notify:?}");
        assert!(s.market_expire(now_ms() + 2_000).await.unwrap().is_empty(), "settled once");
        let carol_claims = s.market_claims(carol).await.unwrap();
        assert_eq!(carol_claims.last().unwrap().reason, "Won: Pager");
        let alice_claims = s.market_claims(alice).await.unwrap();
        assert!(alice_claims.iter().any(|c| c.reason == "Unsold: Mixtape" && c.item.is_some()));
        assert!(alice_claims.iter().any(|c| c.reason == "Sold: Pager" && c.cash == 12));
    }

    #[tokio::test]
    async fn cars_are_created_moved_and_loaded() {
        let Some(mut s) = store().await else { return };
        let a = account(&mut s, "Dana").await;
        let dana = character(&mut s, a, "Dana").await.unwrap();
        let car = s.create_car(dana, "hatchback", ZoneId(4), Vec2::new(-24.0, 2.0), 0.5).await.unwrap().unwrap();
        assert_eq!((car.owner_name.as_str(), car.model.as_str(), car.zone), ("Dana", "hatchback", ZoneId(4)));
        s.handle(DbCommand::SaveCar { car: car.id, zone: ZoneId(1), position: Vec2::new(40.0, 2.0), heading: 3.0 }, LIMITS).await;
        let cars = s.load_cars().await.unwrap();
        assert_eq!(cars.len(), 1);
        assert_eq!((cars[0].zone, cars[0].position, cars[0].heading), (ZoneId(1), Vec2::new(40.0, 2.0), 3.0));
        assert_eq!((cars[0].paint, cars[0].parts), (None, 0), "factory fresh");
        s.handle(DbCommand::SaveCarLook { car: car.id, paint: Some(3), parts: 0b101 }, LIMITS).await;
        let cars = s.load_cars().await.unwrap();
        assert_eq!((cars[0].paint, cars[0].parts, cars[0].zone), (Some(3), 0b101, ZoneId(1)));
    }

    #[tokio::test]
    async fn bans_and_reports() {
        let Some(mut s) = store().await else { return };
        let a = account(&mut s, "Alice").await;
        let b = account(&mut s, "Bob").await;
        let alice = character(&mut s, a, "Alice").await.unwrap();
        character(&mut s, b, "Bob").await.unwrap();

        // Bans are on the account, found through any of its characters.
        let until = now_ms() + 3_600_000;
        assert!(s.set_ban("bob", Some(until), Some("spam")).await.unwrap());
        assert!(!s.set_ban("nobody", Some(until), None).await.unwrap());
        let creds = s.credentials("Bob").await.unwrap().unwrap();
        assert_eq!(creds.ban_at(now_ms()).map(|(_, r)| r.to_string()), Some("spam".into()));
        assert!(creds.banned_until_ms.is_some_and(|t| (t - until).abs() < 1000));
        assert!(creds.ban_at(until + 1).is_none(), "bans run out");
        assert!(s.set_ban("Bob", None, None).await.unwrap());
        assert!(s.credentials("Bob").await.unwrap().unwrap().ban_at(now_ms()).is_none());

        // Reports: filed, listed with names, resolved exactly once.
        let id = s.file_report(alice, "BOB", "rude", "[say] you suck").await.unwrap().unwrap();
        assert!(s.file_report(alice, "nobody", "x", "").await.unwrap().is_none());
        let open = s.open_reports().await.unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!((open[0].id, open[0].reporter.as_str(), open[0].target.as_str(), open[0].context.as_str()), (id, "Alice", "Bob", "[say] you suck"));
        assert!(s.resolve_report(id, "Admin").await.unwrap());
        assert!(!s.resolve_report(id, "Admin").await.unwrap());
        assert!(s.open_reports().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn shop_stock_survives_and_only_touched_rows_change() {
        let Some(s) = store().await else { return };
        let row = |shop: &str, stock: Option<(u32, i64)>, demand: Option<(f32, i64)>| ShopRow { shop: shop.into(), item: "soda".into(), stock, demand };
        s.save_shops(&[row("corner", Some((3, 1_000)), None), row("diner", None, Some((4.5, 2_000)))]).await.unwrap();
        s.save_shops(&[row("corner", Some((2, 5_000)), Some((1.0, 5_000)))]).await.unwrap();
        let mut rows = s.load_shops().await.unwrap();
        rows.sort_by(|a, b| a.shop.cmp(&b.shop));
        assert_eq!(rows, vec![row("corner", Some((2, 5_000)), Some((1.0, 5_000))), row("diner", None, Some((4.5, 2_000)))]);
    }

    #[tokio::test]
    async fn houses_are_bought_and_sold() {
        let Some(mut s) = store().await else { return };
        let a = account(&mut s, "Hal").await;
        let hal = character(&mut s, a, "Hal").await.unwrap();
        let b = account(&mut s, "Ivy").await;
        let ivy = character(&mut s, b, "Ivy").await.unwrap();
        assert!(s.buy_house(hal, 1, &PlayerState::new(0)).await.unwrap().is_ok());
        assert!(s.sell_house(ivy, 1, &PlayerState::new(999)).await.unwrap().is_err(), "not hers to sell");
        assert!(s.sell_house(hal, 1, &PlayerState::new(300)).await.unwrap().is_ok());
        assert!(s.load_houses().await.unwrap().is_empty());
        assert_eq!(load(&mut s, a, hal).await.character.state.cash, 300, "paid in the same transaction");
        assert!(s.sell_house(hal, 1, &PlayerState::new(600)).await.unwrap().is_err(), "can't sell it twice");
        assert!(s.buy_house(ivy, 1, &PlayerState::new(0)).await.unwrap().is_ok(), "back on the market");
    }

    #[tokio::test]
    async fn deleting_an_account_tidies_up_after_it() {
        let Some(mut s) = store().await else { return };
        let a = account(&mut s, "Alice").await;
        let b = account(&mut s, "Bob").await;
        let alice = character(&mut s, a, "Alice").await.unwrap();
        let bob = character(&mut s, b, "Bob").await.unwrap();
        // Alice leads a crew Bob is in, and sells a walkman Bob has bid on.
        let crew = s.create_crew(alice, "Wharf Rats", "WRC").await.unwrap().unwrap();
        s.handle(DbCommand::JoinCrew { link: Entity::PLACEHOLDER, character_id: bob, crew_id: crew }, LIMITS).await;
        let st = PlayerState::new(100);
        let listing = NewListing {
            stack: ItemStack { item: "walkman".into(), qty: 1 },
            item_name: "Walkman".into(),
            start_price: 10,
            buyout: None,
            expires_at_ms: now_ms() + 60_000,
            max_active: 10,
        };
        assert!(s.market_create(alice, &st, &listing).await.unwrap().is_ok());
        let id = s.market_search("walk", 0).await.unwrap().0[0].id;
        assert!(s.market_bid(bob, &st, id, 15).await.unwrap().0.is_ok());

        assert!(s.delete_account(a).await.unwrap());
        assert!(!s.delete_account(a).await.unwrap(), "already gone");
        assert!(s.credentials("Alice").await.unwrap().is_none());
        // Bob gets his bid back, and the crew is his now.
        let claims = s.market_claims(bob).await.unwrap();
        assert_eq!((claims[0].cash, claims[0].reason.as_str()), (15, "Refund: Walkman (the seller left)"));
        let roster = s.load_crew(crew).await.unwrap().unwrap();
        assert_eq!(roster.members, vec![(bob, "Bob".to_string(), CrewRank::Leader)]);
        assert!(s.market_search("", 0).await.unwrap().0.is_empty(), "her listing went with her");
        // The name is free again.
        assert!(s.create_account("alice", "hash").await.unwrap().is_some());
    }
    };
}

pub(crate) use store_tests;
