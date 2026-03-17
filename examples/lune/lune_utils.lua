---@meta

---
---     A struct that can be easily shared, stored in Lua app data,
---     and that also guarantees the values are valid OS strings
---     that can be used for process arguments.
---
---     Usable directly from Lua, implementing both `FromLua` and `LuaUserData`.
---
---     Also provides convenience methods for working with the arguments
---     as either `OsString` or `Vec<u8>`, where using the latter implicitly
---     converts to an `OsString` and fails if the conversion is not possible.
---@class ProcessArgs
local ProcessArgs = {}

---@return integer
function ProcessArgs:__len() end

---@param index integer
---@return string?
function ProcessArgs:__index(index) end

function ProcessArgs:__newindex() end

---@return Lua
function ProcessArgs:__iter() end

---
---     A struct that can be easily shared, stored in Lua app data,
---     and that also guarantees the pairs are valid OS strings
---     that can be used for process environment variables.
---
---     Usable directly from Lua, implementing both `FromLua` and `LuaUserData`.
---
---     Also provides convenience methods for working with the variables
---     as either `OsString` or `Vec<u8>`, where using the latter implicitly
---     converts to an `OsString` and fails if the conversion is not possible.
---@class ProcessEnv
local ProcessEnv = {}

---@return integer
function ProcessEnv:__len() end

---@param key any
---@return string?
function ProcessEnv:__index(key) end

---@param key any
---@param val any?
function ProcessEnv:__newindex(key, val) end

---@return Lua
function ProcessEnv:__iter() end
