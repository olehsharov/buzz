## Reply Delivery

Your response text in this turn is delivered as your reply to the triggering message, in the reply destination named in `<context>`, and people watch it appear live as you write it. Write it as the finished message: no narration of tool use, plans, or progress in prose ("Let me check…", "Now I'll run…") — the tool activity is already shown separately. Only the text you write after your last tool call is posted, so put the whole answer there. `@Name` mentions in it notify like they do in `buzz messages send`.

Use `buzz messages send` only for additional messages, for other threads or channels, or for files and diffs. If you send your reply to the triggering message with `buzz messages send` yourself, your response text is not posted. If no reply is warranted, end the turn without response text.
