---@meta

--- The current processing status.
---@alias Status "pending" | "running" | "completed" | "failed"

--- A 2D vector for spatial calculations.
---@class Vec2
---@field x number
---@field y number
local Vec2 = {}

---@return number
function Vec2:magnitude() end

---@param other_x number
---@param other_y number
---@return number
function Vec2:dot(other_x, other_y) end

---@return any
function Vec2:normalized() end

---@return string
function Vec2:__tostring() end

---@return number
function Vec2:__len() end

--- A configuration holder with many typed fields.
---@class Config
---@field name string (readonly)
---@field max_retries integer (readonly)
---@field timeout number? (readonly)
---@field tags string[] (readonly)
---@field metadata table<string, string> (readonly)
---@field path string (readonly)
---@field enabled boolean (readonly)
---@field data string (readonly)
local Config = {}

---@param index integer
---@return string?
function Config:get_tag(index) end

---@param key string
---@return string?
function Config:get_meta(key) end

---@return integer
function Config:tag_count() end

---@class HttpClient
---@field base_url string (readonly)
local HttpClient = {}

---@async
---@param path string
---@return string
function HttpClient:get(path) end

---@async
---@param path string
---@param body string
---@return string
function HttpClient:post(path, body) end

---@param path string
---@return string
function HttpClient:sync_get(path) end

---@class Rect
---@field x integer (readonly)
---@field y integer (readonly)
---@field w integer (readonly)
---@field h integer (readonly)
---@field left integer (readonly)
---@field right integer (readonly)
---@field top integer (readonly)
---@field bottom integer (readonly)
local Rect = {}

---@return integer
function Rect:area() end

---@param x integer
---@param y integer
---@return boolean
function Rect:contains(x, y) end

---@param x integer
---@return Rect
function Rect.with_x(x) end

---@param y integer
---@return Rect
function Rect.with_y(y) end

---@class Parser
local Parser = {}

---@param input string
---@return integer
---@return string
function Parser:parse(input) end

---@param input string
---@return boolean
---@return string
function Parser:try_parse(input) end

--- A registry that uses FxHashMap for fast lookups and ArcSwap for shared config.
---@class Registry
---@field entries table<string, integer> (readonly)
---@field labels table<string, string[]> (readonly)
---@field active_config string (readonly)
local Registry = {}

---@param key string
---@return integer?
function Registry:get_entry(key) end

---@param key string
---@return string[]?
function Registry:get_labels(key) end

---@param val string
function Registry:set_config(val) end

---@return integer
function Registry:entry_count() end
