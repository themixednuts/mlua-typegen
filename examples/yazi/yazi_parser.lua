---@meta

---@class UpdateYankedIter
---@field cut boolean (readonly)
local UpdateYankedIter = {}

---@return integer
function UpdateYankedIter:__len() end

---@return fun(p1: Iter<any, Url>): integer?, Url?
---@return Iter<any, Url>
---@return nil
function UpdateYankedIter:__pairs() end
