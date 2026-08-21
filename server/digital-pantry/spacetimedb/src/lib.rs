use spacetimedb::{log, reducer, table, Identity, ReducerContext, ScheduleAt, SpacetimeType, Table, Timestamp};

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Where an item lives in the household.
#[derive(Clone, Debug, SpacetimeType)]
pub enum Location {
    Fridge,
    Freezer,
    Pantry,
    Counter,
    Other,
}

impl Default for Location {
    fn default() -> Self { Location::Pantry }
}

/// Lifecycle state of a pantry item.
#[derive(Clone, Debug, SpacetimeType, PartialOrd, PartialEq, Ord, Eq)]
pub enum ItemStatus {
    Unopened,
    Opened,
    ExpiringSoon,
    Depleted,
}

impl Default for ItemStatus {
    fn default() -> Self { ItemStatus::Unopened }
}

/// How far ahead of expiry an item is flagged "expiring soon" (seconds).
/// An item is promoted to `ExpiringSoon` when it is within this window of its
/// `est_expiry_ts` — which also covers items that have already passed expiry
/// (their `est_expiry_ts - now` is negative, hence `<=` the window).
const EXPIRY_WARN_SECS: i64 = 3 * 86_400; // 3 days

// ─────────────────────────────────────────────────────────────────────────────
// Tables
// ─────────────────────────────────────────────────────────────────────────────

/// A registered household member. Created automatically on first connect.
#[derive(Clone)]
#[table(accessor = user, public)]
pub struct User {
    #[primary_key]
    pub identity: Identity,
    /// Display name shown in the web UI and digests.
    pub name: String,
    /// Whether this user has ever connected (drives onboarding UI).
    pub is_active: bool,
}

/// A single unit of food in the pantry.
///
/// Granularity: one row = one trackable physical unit. A 24-pack of eggs is
/// one row with `quantity = 24`; a partially-used loaf is one row with
/// `quantity` decremented as it's consumed.
#[derive(Clone)]
#[table(accessor = item, public)]
pub struct Item {
    #[primary_key]
    #[auto_inc]
    pub item_id: u64,

    // ── Identity ──
    /// Normalised, lowercase name used for fuzzy matching ("whole milk").
    #[index(btree)]
    pub name: String,
    /// Human-friendly name as entered ("Whole Milk 1L").
    pub display_name: String,
    /// Barcode (Open Food Facts code) if known, else empty string.
    #[index(btree)]
    pub barcode: String,

    // ── Quantity ──
    /// Current quantity. 0 = depleted. Use with `unit`.
    pub quantity: f64,
    /// Unit string: "pcs", "g", "ml", "L", "units", "loaf", etc.
    pub unit: String,

    // ── Storage ──
    #[index(btree)]
    pub location: Location,
    pub status: ItemStatus,

    // ── Expiration ──
    /// Unix timestamp (seconds) when the item is expected to expire.
    /// 0 = unknown / no expiry.
    #[index(btree)]
    pub est_expiry_ts: i64,
    /// Estimated shelf-life in days (unopened). 0 = unknown.
    pub unopened_days: i32,
    /// Estimated shelf-life in days once opened. 0 = unknown.
    pub opened_days: i32,

    // ── Financial ──
    /// Price paid (in the household currency). 0 = unknown.
    pub price: f64,
    /// Currency code, e.g. "USD", "EUR". Empty = household default.
    pub currency: String,

    // ── Provenance ──
    /// item_id of the receipt this item came from, 0 = manual entry.
    pub source_receipt_id: u64,
    /// Identity of the user who added this item.
    pub added_by: Identity,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A parsed grocery receipt.
#[derive(Clone)]
#[table(accessor = receipt, public)]
pub struct Receipt {
    #[primary_key]
    #[auto_inc]
    pub receipt_id: u64,

    /// Store name as OCR'd ("Kroger", "Whole Foods", etc.).
    pub store_name: String,
    /// Purchase date as a Unix timestamp (seconds). 0 = unknown.
    pub purchased_at: i64,

    /// Total amount (before tax).
    pub total: f64,
    /// Tax amount.
    pub tax: f64,
    /// Currency code.
    pub currency: String,

    /// Raw OCR text (for debugging / re-parsing).
    pub raw_ocr_text: String,
    /// URL to the stored receipt image (if uploaded).
    pub image_url: String,

    /// Identity of the user who submitted this receipt.
    pub added_by: Identity,
    pub created_at: Timestamp,
}

/// A line item on a receipt.
#[derive(Clone)]
#[table(accessor = receipt_item, public)]
pub struct ReceiptItem {
    #[primary_key]
    #[auto_inc]
    pub receipt_item_id: u64,

    /// FK → Receipt.receipt_id
    #[index(btree)]
    pub receipt_id: u64,

    /// OCR'd product name ("Organic Whole Milk 1 Gallon").
    pub product_name: String,
    /// Parsed quantity. 0 = 1 (single item).
    pub quantity: f64,
    /// Unit string.
    pub unit: String,
    /// Line price (before tax).
    pub price: f64,
    /// Currency code.
    pub currency: String,

    /// FK → Item.item_id if this line was matched to an existing Item,
    /// 0 = not yet matched / new item to create.
    pub matched_item_id: u64,
}

/// A recipe stored in the pantry.
#[derive(Clone)]
#[table(accessor = recipe, public)]
pub struct Recipe {
    #[primary_key]
    #[auto_inc]
    pub recipe_id: u64,

    pub name: String,
    /// Number of servings this recipe makes.
    pub servings: u8,
    /// Optional instructions (markdown).
    pub instructions: String,
    /// Optional source URL.
    pub source_url: String,
    /// Identity of the user who added this recipe.
    pub added_by: Identity,
    pub created_at: Timestamp,
}

/// An ingredient in a recipe.
#[derive(Clone)]
#[table(accessor = recipe_ingredient, public)]
pub struct RecipeIngredient {
    #[primary_key]
    #[auto_inc]
    pub recipe_ingredient_id: u64,

    /// FK → Recipe.recipe_id
    #[index(btree)]
    pub recipe_id: u64,

    /// Ingredient name (matched against Item.name for substitution).
    pub ingredient_name: String,
    /// Amount needed.
    pub quantity: f64,
    /// Unit string.
    pub unit: String,
    /// If true, this ingredient is optional / can be substituted.
    pub is_optional: bool,
    /// Suggested substitute ingredient name (empty = none).
    pub substitute: String,
}

/// A shopping list entry.
#[derive(Clone)]
#[table(accessor = shopping_list_item, public)]
pub struct ShoppingListItem {
    #[primary_key]
    #[auto_inc]
    pub shopping_item_id: u64,

    /// Normalised product name.
    #[index(btree)]
    pub name: String,
    /// Quantity to buy.
    pub quantity: f64,
    /// Unit string.
    pub unit: String,
    /// If true, this is a staple that auto-reappears after purchase.
    pub is_staple: bool,
    /// Reason this item was added (for UI context).
    pub reason: String,
    /// Identity of the user who added this.
    pub added_by: Identity,
    pub created_at: Timestamp,
}

/// A user's subscription to the weekly expiration digest.
/// Data-driven: adding a new endpoint = inserting a row, not deploying code.
///
/// A user may have MULTIPLE rows (one per channel, e.g. Discord + Telegram).
/// Delivery is addressed by (channel, handle); `identity` is the creator.
#[derive(Clone)]
#[table(accessor = digest_subscription, public)]
pub struct DigestSubscription {
    #[primary_key]
    #[auto_inc]
    pub subscription_id: u64,

    /// Identity of the user who created this subscription.
    pub identity: Identity,
    /// Delivery channel: "discord" | "telegram" | "whatsapp".
    pub channel: String,
    /// Channel-specific handle (e.g. Discord user ID, Telegram chat ID).
    pub handle: String,
    /// Whether this subscription is currently active.
    pub is_active: bool,
    /// Unix timestamp (seconds) of when this subscription was created.
    pub subscribed_at: i64,
}

/// An event log entry for audit trail and analytics.
#[derive(Clone)]
#[table(accessor = pantry_event, public)]
pub struct PantryEvent {
    #[primary_key]
    #[auto_inc]
    pub event_id: u64,

    /// Event type: "item_added" | "item_removed" | "item_depleted"
    /// | "item_quantity_changed" | "recipe_accepted" | "shopping_list_updated"
    /// | "receipt_ingested" | "digest_sent" | "user_joined".
    #[index(btree)]
    pub event_type: String,
    /// FK → Item.item_id (if applicable), 0 = N/A.
    pub item_id: u64,
    /// Human-readable description.
    pub description: String,
    /// Identity of the user who triggered this event.
    pub actor: Identity,
    pub created_at: Timestamp,
}

/// Scheduling table for the "expiring soon" sweep.
///
/// A row here is a timer: when `scheduled_at` fires, SpacetimeDB calls the
/// `sweep_expiring_items` reducer with this row as its argument. A
/// `ScheduleAt::Interval` row fires *repeatedly* (every interval); a
/// `ScheduleAt::Time` row fires *once* at that timestamp.
///
/// We start a 30-minute loop from `init`. Because the sweep reducer is also a
/// normal reducer, it can be called manually by a client for an on-demand sweep.
#[derive(Clone)]
#[table(accessor = expiry_sweep_schedule, scheduled(sweep_expiring_items))]
pub struct ExpirySweepSchedule {
    /// 0 = repeating interval row (created once by init); non-zero = one-shot.
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    /// When the sweep should run (interval or one-shot time).
    pub scheduled_at: ScheduleAt,
}

// ─────────────────────────────────────────────────────────────────────────────
// Reducers
// ─────────────────────────────────────────────────────────────────────────────

#[reducer(init)]
pub fn init(ctx: &ReducerContext) {
    log::info!("Digital Pantry SpacetimeDB module initialized.");
    // Start the "expiring soon" sweep: a repeating 30-minute loop.
    // Inserting a row into the scheduled table is what actually arms the timer,
    // and doing it in init keeps it transactional with module startup.
    //
    // We guard against a pre-existing row (init can re-run on redeploy) so we
    // don't arm a second overlapping loop.
    let already_armed: Vec<ExpirySweepSchedule> = ctx.db.expiry_sweep_schedule().iter().collect();
    if already_armed.is_empty() {
        let interval = std::time::Duration::from_secs(30 * 60); // 30 minutes
        ctx.db.expiry_sweep_schedule().insert(ExpirySweepSchedule {
            scheduled_id: 0, // 0 = repeating interval
            scheduled_at: interval.into(),
        });
        log::info!("Armed expiry sweep: every 30 minutes.");
    }
}

// ── User ─────────────────────────────────────────────────────────────────────

#[reducer(client_connected)]
pub fn client_connected(ctx: &ReducerContext) {
    let identity = ctx.sender();
    match ctx.db.user().identity().find(identity) {
        Some(_) => {
            // Reconnect: mark active
            let user = ctx.db.user().identity().find(identity).unwrap();
            ctx.db.user().identity().update(User {
                is_active: true,
                ..user
            });
        }
        None => {
            ctx.db.user().insert(User {
                identity,
                name: "Anonymous".to_string(),
                is_active: true,
            });
            log::info!("New user connected: {:?}", identity);
        }
    }
}

#[reducer]
pub fn set_user_name(ctx: &ReducerContext, name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Name cannot be empty".to_string());
    }
    let identity = ctx.sender();
    let user = ctx.db.user().identity().find(identity)
        .ok_or("User not found")?;
    ctx.db.user().identity().update(User {
        name: name.clone(),
        ..user
    });
    log::info!("User {:?} set name to '{}'", identity, name);
    Ok(())
}

// ── Items ────────────────────────────────────────────────────────────────────

#[reducer]
pub fn add_item(
    ctx: &ReducerContext,
    name: String,
    display_name: String,
    quantity: f64,
    unit: String,
    location: Location,
    est_expiry_ts: i64,
    unopened_days: i32,
    opened_days: i32,
    price: f64,
    currency: String,
    barcode: String,
    source_receipt_id: u64,
) -> Result<(), String> {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return Err("Item name cannot be empty".to_string());
    }
    if quantity <= 0.0 {
        return Err("Quantity must be > 0".to_string());
    }

    // Fuzzy-match against existing items to avoid duplicates
    let existing: Vec<Item> = ctx.db.item().iter()
        .filter(|i| i.name == name && i.status != ItemStatus::Depleted)
        .collect();

    if let Some(mut existing_item) = existing.into_iter().next() {
        // Merge: add quantity, keep earliest expiry
        let new_qty = existing_item.quantity + quantity;
        let new_expiry = if est_expiry_ts > 0 && existing_item.est_expiry_ts == 0 {
            est_expiry_ts
        } else if existing_item.est_expiry_ts > 0 && est_expiry_ts == 0 {
            existing_item.est_expiry_ts
        } else if est_expiry_ts > 0 && existing_item.est_expiry_ts > 0 {
            std::cmp::min(est_expiry_ts, existing_item.est_expiry_ts)
        } else {
            0
        };
        existing_item.quantity = new_qty;
        existing_item.est_expiry_ts = new_expiry;
        existing_item.updated_at = ctx.timestamp;
        if !barcode.is_empty() { existing_item.barcode = barcode; }
        if price > 0.0 { existing_item.price = price; existing_item.currency = currency; }
        if source_receipt_id > 0 { existing_item.source_receipt_id = source_receipt_id; }
        ctx.db.item().item_id().update(existing_item);
        log::info!("Merged item '{}' qty to {}", name, new_qty);
    } else {
        let item = Item {
            item_id: 0, // auto_inc
            name: name.clone(),
            display_name,
            barcode,
            quantity,
            unit,
            location,
            status: ItemStatus::Unopened,
            est_expiry_ts,
            unopened_days,
            opened_days,
            price,
            currency,
            source_receipt_id,
            added_by: ctx.sender(),
            created_at: ctx.timestamp,
            updated_at: ctx.timestamp,
        };
        ctx.db.item().insert(item);
        log::info!("Added item '{}' qty {} {}", name, quantity, "units");
    }
    Ok(())
}

#[reducer]
pub fn remove_item(ctx: &ReducerContext, item_id: u64) -> Result<(), String> {
    let item = ctx.db.item().item_id().find(&item_id)
        .ok_or("Item not found")?;
    ctx.db.item().item_id().delete(&item_id);
    log_event(ctx, "item_removed", item_id,
        format!("Removed '{}'", item.display_name));
    Ok(())
}

#[reducer]
pub fn deplete_item(ctx: &ReducerContext, item_id: u64) -> Result<(), String> {
    let item = ctx.db.item().item_id().find(&item_id)
        .ok_or("Item not found")?;
    let display_name = item.display_name.clone();
    ctx.db.item().item_id().update(Item {
        quantity: 0.0,
        status: ItemStatus::Depleted,
        updated_at: ctx.timestamp,
        ..item
    });
    log_event(ctx, "item_depleted", item_id,
        format!("Depleted '{}'", display_name));
    Ok(())
}

#[reducer]
pub fn update_item_quantity(
    ctx: &ReducerContext,
    item_id: u64,
    new_quantity: f64,
) -> Result<(), String> {
    let item = ctx.db.item().item_id().find(&item_id)
        .ok_or("Item not found")?;
    if new_quantity < 0.0 {
        return Err("Quantity cannot be negative".to_string());
    }
    let new_status = if new_quantity == 0.0 {
        ItemStatus::Depleted
    } else if item.status == ItemStatus::Unopened && item.opened_days > 0 {
        ItemStatus::Opened
    } else {
        item.status
    };
    let display_name = item.display_name.clone();
    ctx.db.item().item_id().update(Item {
        quantity: new_quantity,
        status: new_status,
        updated_at: ctx.timestamp,
        ..item
    });
    log_event(ctx, "item_quantity_changed", item_id,
        format!("Updated '{}' to {}", display_name, new_quantity));
    Ok(())
}

#[reducer]
pub fn update_item_location(
    ctx: &ReducerContext,
    item_id: u64,
    location: Location,
) -> Result<(), String> {
    let item = ctx.db.item().item_id().find(&item_id)
        .ok_or("Item not found")?;
    ctx.db.item().item_id().update(Item {
        location,
        updated_at: ctx.timestamp,
        ..item
    });
    Ok(())
}

// ── Receipts ─────────────────────────────────────────────────────────────────

#[reducer]
pub fn add_receipt(
    ctx: &ReducerContext,
    store_name: String,
    purchased_at: i64,
    total: f64,
    tax: f64,
    currency: String,
    raw_ocr_text: String,
    image_url: String,
) -> Result<(), String> {
    let receipt = Receipt {
        receipt_id: 0, // auto_inc
        store_name: store_name.clone(),
        purchased_at,
        total,
        tax,
        currency,
        raw_ocr_text,
        image_url,
        added_by: ctx.sender(),
        created_at: ctx.timestamp,
    };
    let r = ctx.db.receipt().insert(receipt);
    log_event(ctx, "receipt_ingested", r.receipt_id,
        format!("Ingested receipt from {}", r.store_name));
    Ok(())
}

#[reducer]
pub fn add_receipt_item(
    ctx: &ReducerContext,
    receipt_id: u64,
    product_name: String,
    quantity: f64,
    unit: String,
    price: f64,
    currency: String,
) -> Result<(), String> {
    if ctx.db.receipt().receipt_id().find(&receipt_id).is_none() {
        return Err(format!("Receipt {} not found", receipt_id));
    }
    ctx.db.receipt_item().insert(ReceiptItem {
        receipt_item_id: 0, // auto_inc
        receipt_id,
        product_name,
        quantity,
        unit,
        price,
        currency,
        matched_item_id: 0,
    });
    Ok(())
}

#[reducer]
pub fn match_receipt_item(
    ctx: &ReducerContext,
    receipt_item_id: u64,
    item_id: u64,
) -> Result<(), String> {
    let ri = ctx.db.receipt_item().receipt_item_id().find(&receipt_item_id)
        .ok_or("Receipt item not found")?;
    ctx.db.receipt_item().receipt_item_id().update(ReceiptItem {
        matched_item_id: item_id,
        ..ri
    });
    Ok(())
}

// ── Recipes ──────────────────────────────────────────────────────────────────

#[reducer]
pub fn add_recipe(
    ctx: &ReducerContext,
    name: String,
    servings: u8,
    instructions: String,
    source_url: String,
) -> Result<(), String> {
    let recipe = Recipe {
        recipe_id: 0, // auto_inc
        name,
        servings,
        instructions,
        source_url,
        added_by: ctx.sender(),
        created_at: ctx.timestamp,
    };
    ctx.db.recipe().insert(recipe);
    Ok(())
}

#[reducer]
pub fn add_recipe_ingredient(
    ctx: &ReducerContext,
    recipe_id: u64,
    ingredient_name: String,
    quantity: f64,
    unit: String,
    is_optional: bool,
    substitute: String,
) -> Result<(), String> {
    if ctx.db.recipe().recipe_id().find(&recipe_id).is_none() {
        return Err(format!("Recipe {} not found", recipe_id));
    }
    ctx.db.recipe_ingredient().insert(RecipeIngredient {
        recipe_ingredient_id: 0, // auto_inc
        recipe_id,
        ingredient_name,
        quantity,
        unit,
        is_optional,
        substitute,
    });
    Ok(())
}

/// Accept a recipe: deducts each non-optional ingredient from the pantry.
/// Ingredients with no matching in-stock item are reported in the event log
/// so the UI can surface "you're missing X" without a second round-trip.
#[reducer]
pub fn accept_recipe(ctx: &ReducerContext, recipe_id: u64) -> Result<(), String> {
    let recipe = ctx.db.recipe().recipe_id().find(&recipe_id)
        .ok_or("Recipe not found")?;

    let ingredients: Vec<RecipeIngredient> = ctx.db.recipe_ingredient().iter()
        .filter(|ri| ri.recipe_id == recipe_id && !ri.is_optional)
        .collect();

    let mut missing: Vec<String> = vec![];
    let mut deducted: Vec<String> = vec![];

    for ing in &ingredients {
        // Find matching item by normalised name
        let normalized = ing.ingredient_name.to_lowercase();
        let item: Option<Item> = ctx.db.item().iter()
            .filter(|i| i.name == normalized && i.status != ItemStatus::Depleted && i.quantity >= ing.quantity)
            .next();

        match item {
            Some(mut matched) => {
                matched.quantity -= ing.quantity;
                matched.updated_at = ctx.timestamp;
                if matched.quantity <= 0.0 {
                    matched.status = ItemStatus::Depleted;
                    matched.quantity = 0.0;
                }
                ctx.db.item().item_id().update(matched);
                deducted.push(format!("{} ({})", ing.ingredient_name, ing.quantity));
            }
            None => {
                missing.push(ing.ingredient_name.clone());
            }
        }
    }

    if !missing.is_empty() {
        log_event(ctx, "recipe_accepted", recipe_id,
            format!("Recipe '{}' accepted. Missing: {}", recipe.name, missing.join(", ")));
    } else {
        log_event(ctx, "recipe_accepted", recipe_id,
            format!("Recipe '{}' accepted. Deducted: {}", recipe.name, deducted.join(", ")));
    }
    Ok(())
}

// ── Shopping List ────────────────────────────────────────────────────────────

#[reducer]
pub fn add_shopping_item(
    ctx: &ReducerContext,
    name: String,
    quantity: f64,
    unit: String,
    is_staple: bool,
    reason: String,
) -> Result<(), String> {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return Err("Shopping item name cannot be empty".to_string());
    }
    ctx.db.shopping_list_item().insert(ShoppingListItem {
        shopping_item_id: 0, // auto_inc
        name,
        quantity,
        unit,
        is_staple,
        reason,
        added_by: ctx.sender(),
        created_at: ctx.timestamp,
    });
    Ok(())
}

#[reducer]
pub fn remove_shopping_item(ctx: &ReducerContext, shopping_item_id: u64) -> Result<(), String> {
    let item = ctx.db.shopping_list_item().shopping_item_id().find(&shopping_item_id)
        .ok_or("Shopping item not found")?;
    ctx.db.shopping_list_item().shopping_item_id().delete(&shopping_item_id);
    log_event(ctx, "shopping_list_updated", item.shopping_item_id,
        format!("Removed '{}' from shopping list", item.name));
    Ok(())
}

// ── Digest Subscriptions ─────────────────────────────────────────────────────

/// Subscribe (or re-subscribe) to the digest on a given channel/handle.
/// Dedup: if an active row for this (identity, channel, handle) already
/// exists it is refreshed in place; otherwise a new row is created.
/// A user may hold several rows — one per endpoint.
#[reducer]
pub fn subscribe_digest(
    ctx: &ReducerContext,
    channel: String,
    handle: String,
) -> Result<(), String> {
    if channel.is_empty() || handle.is_empty() {
        return Err("Channel and handle are required".to_string());
    }
    let identity = ctx.sender();
    let ts = now_ts(ctx);
    let existing: Vec<DigestSubscription> = ctx.db.digest_subscription().iter()
        .filter(|s| s.identity == identity && s.channel == channel && s.handle == handle)
        .collect();

    match existing.into_iter().next() {
        Some(sub) => {
            ctx.db.digest_subscription().subscription_id().update(DigestSubscription {
                is_active: true,
                subscribed_at: ts,
                ..sub
            });
        }
        None => {
            ctx.db.digest_subscription().insert(DigestSubscription {
                subscription_id: 0, // auto_inc
                identity,
                channel,
                handle,
                is_active: true,
                subscribed_at: ts,
            });
        }
    }
    Ok(())
}

#[reducer]
pub fn unsubscribe_digest(ctx: &ReducerContext, subscription_id: u64) -> Result<(), String> {
    let sub = ctx.db.digest_subscription().subscription_id().find(&subscription_id)
        .ok_or("Subscription not found")?;
    ctx.db.digest_subscription().subscription_id().update(DigestSubscription {
        is_active: false,
        ..sub
    });
    Ok(())
}

// ── Expiry sweep (scheduled) ─────────────────────────────────────────────────

/// Scheduled sweep that promotes items to `ExpiringSoon`.
///
/// Runs on a 30-minute interval (armed from `init` via `ExpirySweepSchedule`).
/// For every item that is still in stock with a known expiry, if it is within
/// `EXPIRY_WARN_SECS` of expiring (or already past expiry), and it is not
/// already flagged, promote it and log the transition.
///
/// Idempotent: once an item is `ExpiringSoon`, the `status < ExpiringSoon`
/// check stops it being re-flagged. Depleted items are skipped, so the flag
/// never clutters the list of things already gone.
///
/// Because this is also a normal reducer, a client can call it directly to force
/// an on-demand sweep (e.g. a "refresh" button in the UI).
#[reducer]
pub fn sweep_expiring_items(
    ctx: &ReducerContext,
    _arg: ExpirySweepSchedule,
) {
    let now = now_ts(ctx);
    let mut promoted = 0usize;
    for item in ctx.db.item().iter() {
        // Only consider items still in stock, not already flagged, with a
        // known expiry. `status < ExpiringSoon` is true only for Unopened/Opened
        // (enum derives Ord in declaration order).
        if item.quantity > 0.0
            && item.est_expiry_ts > 0
            && item.status < ItemStatus::ExpiringSoon
        {
            let secs_to_expiry = item.est_expiry_ts - now;
            if secs_to_expiry <= EXPIRY_WARN_SECS {
                let display = item.display_name.clone();
                let status = ItemStatus::ExpiringSoon;
                ctx.db.item().item_id().update(Item {
                    status,
                    updated_at: ctx.timestamp,
                    ..item
                });
                let days_left = (secs_to_expiry / 86_400).max(0);
                log_event(
                    ctx,
                    "item_expiring_soon",
                    item.item_id,
                    if secs_to_expiry <= 0 {
                        format!("'{}' has passed its estimated expiry", display)
                    } else {
                        format!("'{}' expires in ~{} day(s)", display, days_left)
                    },
                );
                promoted += 1;
            }
        }
    }
    if promoted > 0 {
        log::info!("Expiry sweep: promoted {} item(s) to ExpiringSoon.", promoted);
    }
}

// ── Events (internal helper) ─────────────────────────────────────────────────

fn log_event(
    ctx: &ReducerContext,
    event_type: &str,
    item_id: u64,
    description: String,
) {
    ctx.db.pantry_event().insert(PantryEvent {
        event_id: 0, // auto_inc
        event_type: event_type.to_string(),
        item_id,
        description,
        actor: ctx.sender(),
        created_at: ctx.timestamp,
    });
}

/// Returns current Unix timestamp in seconds (from the reducer context).
fn now_ts(ctx: &ReducerContext) -> i64 {
    ctx.timestamp.to_micros_since_unix_epoch() / 1_000_000
}
