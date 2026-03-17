---@meta

---
---     Wrapper struct to accept either a Lua thread or a Lua function as function argument.
---
---     [`LuaThreadOrFunction::into_thread`] may be used to convert the value into a Lua thread.
---@alias LuaThreadOrFunction "thread" | "function"
