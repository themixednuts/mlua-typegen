---@meta

---@class Core
local Core = {}

---@param key string
---@return (TO_DESTROY | Layer)?
function Core:__index(key) end

---@class File
---@field cha Cha (readonly)
---@field url Url (readonly)
---@field link_to Path? (readonly)
---@field name string? (readonly)
---@field path Path (readonly)
---@field cache Path? (readonly)
---@field bare File (readonly)
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
---@field filter Filter (readonly)
local Files = {}

---@return integer
function Files:__len() end

---@param idx integer
---@return File
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
---@field cwd Url (readonly)
---@field files Files (readonly)
---@field stage FolderStage (readonly)
---@field window Files (readonly)
---@field offset integer (readonly)
---@field cursor integer (readonly)
---@field hovered File (readonly)
local Folder = {}

---@class Mode
---@field is_select boolean (readonly)
---@field is_unset boolean (readonly)
---@field is_visual boolean (readonly)
local Mode = {}

---@return string
function Mode:__tostring() end

---@class Preference
---@field name string (readonly)
---@field linemode string (readonly)
---@field show_hidden boolean (readonly)
---@field sort_by string (readonly)
---@field sort_sensitive boolean (readonly)
---@field sort_reverse boolean (readonly)
---@field sort_dir_first boolean (readonly)
---@field sort_translit boolean (readonly)
---@field sort_fallback string (readonly)
local Preference = {}

---@class Preview
---@field skip integer (readonly)
---@field folder Folder (readonly)
local Preview = {}

---@class Tab
---@field id Id (readonly)
---@field name string (readonly)
---@field mode Mode (readonly)
---@field pref Preference (readonly)
---@field current Folder (readonly)
---@field parent Folder (readonly)
---@field selected Selected (readonly)
---@field preview Preview (readonly)
---@field finder Finder (readonly)
local Tab = {}

---@param url Url
---@return Folder
function Tab:history(url) end

---@class Tabs
---@field idx integer (readonly)
local Tabs = {}

---@return integer
function Tabs:__len() end

---@param idx integer
---@return Tab
function Tabs:__index(idx) end

---@class TaskSnap
---@field name string (readonly)
---@field prog any (readonly)
---@field cooked boolean (readonly)
---@field running boolean (readonly)
---@field success boolean (readonly)
---@field failed boolean (readonly)
---@field percent number? (readonly)
local TaskSnap = {}

---@class Tasks
---@field cursor integer (readonly)
---@field snaps table (readonly)
---@field summary any (readonly)
local Tasks = {}

---@class Which
---@field tx MpscUnboundedTx? (readonly)
---@field times integer (readonly)
---@field cands table (readonly)
---@field active boolean (readonly)
---@field silent boolean (readonly)
local Which = {}

---@class Yanked
---@field is_cut boolean (readonly)
local Yanked = {}

---@return integer
function Yanked:__len() end

---@return fun(p1: Iter<any, Url>): integer?, Url?
---@return Iter<any, Url>
---@return nil
function Yanked:__pairs() end
