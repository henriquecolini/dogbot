// @generated automatically by Diesel CLI.

diesel::table! {
    chats (id) {
        id -> Int8,
        name -> Nullable<Text>,
        alias -> Nullable<Text>,
        created_at -> Timestamptz,
        is_group -> Bool,
    }
}

diesel::table! {
    files (id) {
        id -> Uuid,
        chat_id -> Int8,
        owner_id -> Nullable<Int8>,
        parent_id -> Nullable<Uuid>,
        name -> Text,
        others_read -> Bool,
        others_write -> Bool,
        content -> Nullable<Bytea>,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    messages (id, chat_id) {
        id -> Int8,
        chat_id -> Int8,
        user_id -> Nullable<Int8>,
        content -> Text,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    users (id) {
        id -> Int8,
        first_name -> Text,
        last_name -> Nullable<Text>,
        username -> Nullable<Text>,
        created_at -> Timestamptz,
        current_connection -> Nullable<Int8>,
    }
}

diesel::table! {
    users_in_chats (user_id, chat_id) {
        user_id -> Int8,
        chat_id -> Int8,
        is_admin -> Bool,
        created_at -> Timestamptz,
    }
}

diesel::joinable!(files -> chats (chat_id));
diesel::joinable!(files -> users (owner_id));
diesel::joinable!(messages -> chats (chat_id));
diesel::joinable!(messages -> users (user_id));
diesel::joinable!(users_in_chats -> chats (chat_id));

diesel::allow_tables_to_appear_in_same_query!(chats, files, messages, users, users_in_chats,);
