---@meta

---@class Core
local Core = {}

---@param key string
---@return any
function Core:__index(key) end

---@class File
---@field cha any (readonly)
---@field url any (readonly)
---@field link_to any (readonly)
---@field name any (readonly)
---@field path any (readonly)
---@field cache any (readonly)
---@field bare any (readonly)
---@field idx integer (readonly)
---@field is_hovered boolean (readonly)
---@field in_current boolean (readonly)
---@field in_preview boolean (readonly)
local File = {}

---@return integer
function File:hash() end

---@return Icon?
function File:icon() end

---@return integer?
function File:size() end

---@return string?
function File:mime() end

---@return string?
function File:prefix() end

---@return Style?
function File:style() end

---@return integer
function File:is_yanked() end

---@return integer
function File:is_marked() end

---@return boolean
function File:is_selected() end

---@return table?
function File:found() end

---@return Range[]?
function File:highlights() end

---@class Files
---@field filter any (readonly)
local Files = {}

---@return integer
function Files:__len() end

---@param idx integer
---@return any?
function Files:__index(idx) end

---@class Filter
local Filter = {}

---@return string
function Filter:__tostring() end

---@class Finder
local Finder = {}

---@return string
function Finder:__tostring() end

---@class Folder
---@field cwd any (readonly)
---@field files any (readonly)
---@field stage any (readonly)
---@field window any (readonly)
---@field offset integer (readonly)
---@field cursor integer (readonly)
---@field hovered any (readonly)
local Folder = {}

---@class Mode
---@field is_select boolean (readonly)
---@field is_unset boolean (readonly)
---@field is_visual boolean (readonly)
local Mode = {}

---@return string
function Mode:__tostring() end

---@class Preference
---@field name any (readonly)
---@field linemode any (readonly)
---@field show_hidden boolean (readonly)
---@field sort_by any (readonly)
---@field sort_sensitive boolean (readonly)
---@field sort_reverse boolean (readonly)
---@field sort_dir_first boolean (readonly)
---@field sort_translit boolean (readonly)
---@field sort_fallback string (readonly)
local Preference = {}

---@class Preview
---@field skip integer (readonly)
---@field folder any (readonly)
local Preview = {}

---@class Tab
---@field id Id (readonly)
---@field name any (readonly)
---@field mode any (readonly)
---@field pref any (readonly)
---@field current any (readonly)
---@field parent any (readonly)
---@field selected any (readonly)
---@field preview any (readonly)
---@field finder any (readonly)
local Tab = {}

---@param url UserDataRef
---@return any?
function Tab:history(url) end

---@class Tabs
---@field idx integer (readonly)
local Tabs = {}

---@return integer
function Tabs:__len() end

---@param idx integer
---@return any?
function Tabs:__index(idx) end

---@class TaskSnap
---@field name any (readonly)
---@field prog any (readonly)
---@field cooked boolean (readonly)
---@field running boolean (readonly)
---@field success boolean (readonly)
---@field failed boolean (readonly)
---@field percent number? (readonly)
local TaskSnap = {}

---@class Tasks
---@field cursor integer (readonly)
---@field snaps any (readonly)
---@field summary any (readonly)
local Tasks = {}

---@class Which
---@field tx any (readonly)
---@field times integer (readonly)
---@field cands any (readonly)
---@field active boolean (readonly)
---@field silent boolean (readonly)
local Which = {}

---@class Yanked
---@field is_cut boolean (readonly)
local Yanked = {}

---@return integer
function Yanked:__len() end

---@return any
function Yanked:__pairs() end
