---@meta

---
---     A wrapper over the `regex::Captures` struct that can be used from Lua.
---@class LuaCaptures
local LuaCaptures = {}

---@param index integer
---@return LuaMatch?
function LuaCaptures:get(index) end

---@param group string
---@return LuaMatch?
function LuaCaptures:group(group) end

---@param format string
---@return string
function LuaCaptures:format(format) end

---@return integer
function LuaCaptures:__len() end

---@return string
function LuaCaptures:__tostring() end

---
---     A wrapper over the `regex::Match` struct that can be used from Lua.
---@class LuaMatch
---@field start integer (readonly)
---@field finish integer (readonly)
---@field len integer (readonly)
---@field text string (readonly)
local LuaMatch = {}

---@return integer
function LuaMatch:__len() end

---@return string
function LuaMatch:__tostring() end

---
---     A wrapper over the `regex::Regex` struct that can be used from Lua.
---@class LuaRegex
local LuaRegex = {}

---@param text string
---@return boolean
function LuaRegex:isMatch(text) end

---@param text string
---@return LuaMatch?
function LuaRegex:find(text) end

---@param text string
---@return LuaCaptures?
function LuaRegex:captures(text) end

---@param text string
---@return string[]
function LuaRegex:split(text) end

---@param haystack string
---@param replacer string
---@return string
function LuaRegex:replace(haystack, replacer) end

---@param haystack string
---@param replacer string
---@return string
function LuaRegex:replaceAll(haystack, replacer) end

---@return string
function LuaRegex:__tostring() end
