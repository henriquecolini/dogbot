// @generated automatically by Diesel CLI.

diesel::table! {
    chats (id) {
        id -> Int8,
        name -> Nullable<Text>,
        alias -> Nullable<Text>,
        created_at -> Timestamptz,
        is_group -> Bool,
        root_id -> Uuid,
    }
}

diesel::table! {
    files (id) {
        id -> Uuid,
        chat_id -> Int8,
        owner_id -> Nullable<Int8>,
        parent_id -> Uuid,
        name -> Text,
        group_read -> Bool,
        group_write -> Bool,
        content -> Nullable<Bytea>,
        created_at -> Timestamptz,
        group_execute -> Bool,
        user_read -> Bool,
        user_write -> Bool,
        user_execute -> Bool,
        others_read -> Bool,
        others_write -> Bool,
        others_execute -> Bool,
        last_modified_at -> Timestamptz,
        is_dir -> Bool,
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

diesel::joinable!(files -> users (owner_id));
diesel::joinable!(messages -> chats (chat_id));
diesel::joinable!(messages -> users (user_id));
diesel::joinable!(users_in_chats -> chats (chat_id));

diesel::allow_tables_to_appear_in_same_query!(chats, files, messages, users, users_in_chats,);
