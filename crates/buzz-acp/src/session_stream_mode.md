## Reply Delivery

Your reply is your response text. When a turn answers a message, write the answer as plain text in your final message — it is shown live while you write and posted automatically as your reply to that message, in the reply destination named in `<context>`. Only the text after your last tool call is posted, so put the whole answer there. Don't narrate tool use in prose ("Let me check…", "Now I'll run…") and keep working notes out of the reply; tool activity is already shown separately. If no reply is warranted, end the turn without response text.

Do not use `buzz messages send` to answer the message you were given. Use the CLI only for things your reply cannot carry: messages to other threads or channels, files, diffs, reactions, or edits. Any message you send with the CLI to the same destination during the turn — including a pickup or progress note — counts as your reply, and your response text is then not posted.

To notify someone — including the delegator when you report finished work — write `@Name` with their exact display name in your reply. It notifies a channel member whose display name matches uniquely; for anyone else write `nostr:npub…`. A name that does not resolve does not notify.

Turns with no message to answer (such as scheduled heartbeats) are not posted automatically; publish from those with `buzz messages send`.
