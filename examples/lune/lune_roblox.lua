---@meta

---@alias FontWeight "thin" | "extra_light" | "light" | "regular" | "medium" | "semi_bold" | "bold" | "extra_bold" | "heavy"

---@alias FontStyle "normal" | "italic"

---
---     An implementation of the [Axes](https://create.roblox.com/docs/reference/engine/datatypes/Axes) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the Axes class as of October 2025.
---@class Axes
---@field X boolean (readonly)
---@field Y boolean (readonly)
---@field Z boolean (readonly)
---@field Left boolean (readonly)
---@field Right boolean (readonly)
---@field Top boolean (readonly)
---@field Bottom boolean (readonly)
---@field Front boolean (readonly)
---@field Back boolean (readonly)
local Axes = {}

---
---     An implementation of the [BrickColor](https://create.roblox.com/docs/reference/engine/datatypes/BrickColor) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the `BrickColor` class as of October 2025.
---@class BrickColor
---@field Number integer (readonly)
---@field Name string (readonly)
---@field R number (readonly)
---@field G number (readonly)
---@field B number (readonly)
---@field r number (readonly)
---@field g number (readonly)
---@field b number (readonly)
---@field Color Color3 (readonly)
local BrickColor = {}

---
---     An implementation of the [CFrame](https://create.roblox.com/docs/reference/engine/datatypes/CFrame)
---     Roblox datatype, backed by [`glam::Mat4`].
---
---     This implements most documented properties, methods & constructors of the `CFrame` class as of October 2025,
---     notably missing `CFrame.fromRotationBetweenVectors`, `CFrame:FuzzyEq()`, and `CFrame:AngleBetween()`.
---@class CFrame
---@field Position Vector3 (readonly)
---@field Rotation CFrame (readonly)
---@field X number (readonly)
---@field Y number (readonly)
---@field Z number (readonly)
---@field XVector Vector3 (readonly)
---@field YVector Vector3 (readonly)
---@field ZVector Vector3 (readonly)
---@field RightVector Vector3 (readonly)
---@field UpVector Vector3 (readonly)
---@field LookVector Vector3 (readonly)
local CFrame = {}

---@return CFrame
function CFrame:Inverse() end

---@param goal UserDataRef
---@param alpha number
---@return CFrame
function CFrame:Lerp(goal, alpha) end

---@return CFrame
function CFrame:Orthonormalize() end

---@param ... UserDataRef
---@return CFrame...
function CFrame:ToWorldSpace(...) end

---@param ... UserDataRef
---@return CFrame...
function CFrame:ToObjectSpace(...) end

---@param ... UserDataRef
---@return Vector3...
function CFrame:PointToWorldSpace(...) end

---@param ... UserDataRef
---@return Vector3...
function CFrame:PointToObjectSpace(...) end

---@param ... UserDataRef
---@return Vector3...
function CFrame:VectorToWorldSpace(...) end

---@param ... UserDataRef
---@return Vector3...
function CFrame:VectorToObjectSpace(...) end

---@return number x
---@return number y
---@return number z
---@return number x
---@return number y
---@return number z
---@return number x
---@return number y
---@return number z
---@return number x
---@return number y
---@return number z
function CFrame:GetComponents() end

---@return number
---@return number
---@return number
function CFrame:ToEulerAnglesXYZ() end

---@return number rx
---@return number ry
---@return number rz
function CFrame:ToEulerAnglesYXZ() end

---@return number rx
---@return number ry
---@return number rz
function CFrame:ToOrientation() end

---@return Vector3 Vector3
---@return number angle
function CFrame:ToAxisAngle() end

---@param rhs any
---@return any
function CFrame:__mul(rhs) end

---@param vec UserDataRef
---@return CFrame
function CFrame:__add(vec) end

---@param vec UserDataRef
---@return CFrame
function CFrame:__sub(vec) end

---
---     An implementation of the [Color3](https://create.roblox.com/docs/reference/engine/datatypes/Color3) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the Color3 class as of October 2025.
---
---     It also implements math operations for addition, subtraction, multiplication, and division,
---     all of which are suspiciously missing from the Roblox implementation of the Color3 datatype.
---@class Color3
---@field R number (readonly)
---@field G number (readonly)
---@field B number (readonly)
local Color3 = {}

---@param rhs UserDataRef
---@param alpha number
---@return Color3
function Color3:Lerp(rhs, alpha) end

---@return number hue
---@return number sat
---@return number max
function Color3:ToHSV() end

---@return string
function Color3:ToHex() end

---
---     An implementation of the [ColorSequence](https://create.roblox.com/docs/reference/engine/datatypes/ColorSequence) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the `ColorSequence` class as of October 2025.
---@class ColorSequence
---@field Keypoints ColorSequenceKeypoint[] (readonly)
local ColorSequence = {}

---
---     An implementation of the [ColorSequenceKeypoint](https://create.roblox.com/docs/reference/engine/datatypes/ColorSequenceKeypoint) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the `ColorSequenceKeypoint` class as of October 2025.
---@class ColorSequenceKeypoint
---@field Time number (readonly)
---@field Value Color3 (readonly)
local ColorSequenceKeypoint = {}

---
---     An implementation of the [Content](https://create.roblox.com/docs/reference/engine/datatypes/Content) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the Content type as of October 2025.
---@class Content
---@field SourceType EnumItem? (readonly)
---@field Uri string? (readonly)
---@field Object Instance? (readonly)
local Content = {}

---
---     An implementation of the [Enum](https://create.roblox.com/docs/reference/engine/datatypes/Enum) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the Enum class as of October 2025.
---@class Enum
local Enum = {}

---@return EnumItem[]
function Enum:GetEnumItems() end

---@param name string
---@return EnumItem
function Enum:__index(name) end

---
---     An implementation of the [EnumItem](https://create.roblox.com/docs/reference/engine/datatypes/EnumItem) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the `EnumItem` class as of October 2025.
---@class EnumItem
---@field Name string (readonly)
---@field Value integer (readonly)
---@field EnumType Enum (readonly)
local EnumItem = {}

---
---     An implementation of the [Enums](https://create.roblox.com/docs/reference/engine/datatypes/Enums) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the Enums class as of October 2025.
---@class Enums
local Enums = {}

---@return Enum[]
function Enums:GetEnums() end

---@param name string
---@return Enum
function Enums:__index(name) end

---
---     An implementation of the [Faces](https://create.roblox.com/docs/reference/engine/datatypes/Faces) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the Faces class as of October 2025.
---@class Faces
---@field Right boolean (readonly)
---@field Top boolean (readonly)
---@field Back boolean (readonly)
---@field Left boolean (readonly)
---@field Bottom boolean (readonly)
---@field Front boolean (readonly)
local Faces = {}

---
---     An implementation of the [Font](https://create.roblox.com/docs/reference/engine/datatypes/Font) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the Font class as of October 2025.
---@class Font
---@field Family string (readonly)
---@field Weight FontWeight
---@field Style FontStyle
---@field Bold boolean
local Font = {}

---
---     An implementation of the [NumberRange](https://create.roblox.com/docs/reference/engine/datatypes/NumberRange) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the `NumberRange` class as of October 2025.
---@class NumberRange
---@field Min number (readonly)
---@field Max number (readonly)
local NumberRange = {}

---
---     An implementation of the [NumberSequence](https://create.roblox.com/docs/reference/engine/datatypes/NumberSequence) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the `NumberSequence` class as of October 2025.
---@class NumberSequence
---@field Keypoints NumberSequenceKeypoint[] (readonly)
local NumberSequence = {}

---
---     An implementation of the [NumberSequenceKeypoint](https://create.roblox.com/docs/reference/engine/datatypes/NumberSequenceKeypoint) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the `NumberSequenceKeypoint` class as of October 2025.
---@class NumberSequenceKeypoint
---@field Time number (readonly)
---@field Value number (readonly)
---@field Envelope number (readonly)
local NumberSequenceKeypoint = {}

---
---     An implementation of the [PhysicalProperties](https://create.roblox.com/docs/reference/engine/datatypes/PhysicalProperties) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the `PhysicalProperties` class as of October 2025.
---@class PhysicalProperties
---@field Density number (readonly)
---@field Friction number (readonly)
---@field FrictionWeight number (readonly)
---@field Elasticity number (readonly)
---@field ElasticityWeight number (readonly)
---@field AcousticAbsorption number (readonly)
local PhysicalProperties = {}

---
---     An implementation of the [Ray](https://create.roblox.com/docs/reference/engine/datatypes/Ray)
---     Roblox datatype, backed by [`glam::Vec3`].
---
---     This implements all documented properties, methods & constructors of the Ray class as of October 2025.
---@class Ray
---@field Origin Vector3 (readonly)
---@field Direction Vector3 (readonly)
---@field Unit Ray (readonly)
local Ray = {}

---@param to UserDataRef
---@return Vector3
function Ray:ClosestPoint(to) end

---@param to UserDataRef
---@return number
function Ray:Distance(to) end

---
---     An implementation of the [Rect](https://create.roblox.com/docs/reference/engine/datatypes/Rect)
---     Roblox datatype, backed by [`glam::Vec2`].
---
---     This implements all documented properties, methods & constructors of the Rect class as of October 2025.
---@class Rect
---@field Min Vector2 (readonly)
---@field Max Vector2 (readonly)
---@field Width number (readonly)
---@field Height number (readonly)
local Rect = {}

---
---     An implementation of the [Region3](https://create.roblox.com/docs/reference/engine/datatypes/Region3)
---     Roblox datatype, backed by [`glam::Vec3`].
---
---     This implements all documented properties, methods & constructors of the Region3 class as of October 2025.
---@class Region3
---@field CFrame CFrame (readonly)
---@field Size Vector3 (readonly)
local Region3 = {}

---@param resolution number
---@return Region3
function Region3:ExpandToGrid(resolution) end

---
---     An implementation of the [Region3int16](https://create.roblox.com/docs/reference/engine/datatypes/Region3int16)
---     Roblox datatype, backed by [`glam::IVec3`].
---
---     This implements all documented properties, methods & constructors of the Region3int16 class as of October 2025.
---@class Region3int16
---@field Min Vector3int16 (readonly)
---@field Max Vector3int16 (readonly)
local Region3int16 = {}

---
---     An implementation of the [UDim](https://create.roblox.com/docs/reference/engine/datatypes/UDim) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the `UDim` class as of October 2025.
---@class UDim
---@field Scale number (readonly)
---@field Offset integer (readonly)
local UDim = {}

---
---     An implementation of the [UDim2](https://create.roblox.com/docs/reference/engine/datatypes/UDim2) Roblox datatype.
---
---     This implements all documented properties, methods & constructors of the `UDim2` class as of October 2025.
---@class UDim2
---@field X UDim (readonly)
---@field Y UDim (readonly)
---@field Width UDim (readonly)
---@field Height UDim (readonly)
local UDim2 = {}

---@param goal UserDataRef
---@param alpha number
---@return UDim2
function UDim2:Lerp(goal, alpha) end

---
---     An implementation of the `UniqueId` Roblox datatype.
---
---     This type is not exposed to users in engine by Roblox itself,
---     but is used as an identifier for Instances, and is occasionally
---     useful when manipulating place and model files in Lune.
---@class UniqueId
local UniqueId = {}

---
---     An implementation of the [Vector2](https://create.roblox.com/docs/reference/engine/datatypes/Vector2)
---     Roblox datatype, backed by [`glam::Vec2`].
---
---     This implements all documented properties, methods &
---     constructors of the Vector2 class as of October 2025.
---@class Vector2
---@field Magnitude number (readonly)
---@field Unit Vector2 (readonly)
---@field X number (readonly)
---@field Y number (readonly)
local Vector2 = {}

---@param rhs UserDataRef
---@return number
function Vector2:Angle(rhs) end

---@param rhs UserDataRef
---@return number
function Vector2:Cross(rhs) end

---@param rhs UserDataRef
---@return number
function Vector2:Dot(rhs) end

---@param rhs UserDataRef
---@param epsilon number
---@return boolean
function Vector2:FuzzyEq(rhs, epsilon) end

---@param rhs UserDataRef
---@param alpha number
---@return Vector2
function Vector2:Lerp(rhs, alpha) end

---@param rhs UserDataRef
---@return Vector2
function Vector2:Max(rhs) end

---@param rhs UserDataRef
---@return Vector2
function Vector2:Min(rhs) end

---@return Vector2
function Vector2:Abs() end

---@return Vector2
function Vector2:Ceil() end

---@return Vector2
function Vector2:Floor() end

---@return Vector2
function Vector2:Sign() end

---
---     An implementation of the [Vector2int16](https://create.roblox.com/docs/reference/engine/datatypes/Vector2int16)
---     Roblox datatype, backed by [`glam::IVec2`].
---
---     This implements all documented properties, methods &
---     constructors of the Vector2int16 class as of October 2025.
---@class Vector2int16
---@field X integer (readonly)
---@field Y integer (readonly)
local Vector2int16 = {}

---
---     An implementation of the [Vector3](https://create.roblox.com/docs/reference/engine/datatypes/Vector3)
---     Roblox datatype, backed by [`glam::Vec3`].
---
---     This implements all documented properties, methods &
---     constructors of the Vector3 class as of October 2025.
---
---     Note that this does not use native Luau vectors to simplify implementation
---     and instead allow us to implement all abovementioned APIs accurately.
---@class Vector3
---@field Magnitude number (readonly)
---@field Unit Vector3 (readonly)
---@field X number (readonly)
---@field Y number (readonly)
---@field Z number (readonly)
local Vector3 = {}

---@param rhs UserDataRef
---@return number
function Vector3:Angle(rhs) end

---@param rhs UserDataRef
---@return Vector3
function Vector3:Cross(rhs) end

---@param rhs UserDataRef
---@return number
function Vector3:Dot(rhs) end

---@param rhs UserDataRef
---@param epsilon number
---@return boolean
function Vector3:FuzzyEq(rhs, epsilon) end

---@param rhs UserDataRef
---@param alpha number
---@return Vector3
function Vector3:Lerp(rhs, alpha) end

---@param rhs UserDataRef
---@return Vector3
function Vector3:Max(rhs) end

---@param rhs UserDataRef
---@return Vector3
function Vector3:Min(rhs) end

---@return Vector3
function Vector3:Abs() end

---@return Vector3
function Vector3:Ceil() end

---@return Vector3
function Vector3:Floor() end

---@return Vector3
function Vector3:Sign() end

---
---     An implementation of the [Vector3int16](https://create.roblox.com/docs/reference/engine/datatypes/Vector3int16)
---     Roblox datatype, backed by [`glam::IVec3`].
---
---     This implements all documented properties, methods &
---     constructors of the Vector3int16 class as of October 2025.
---@class Vector3int16
---@field X integer (readonly)
---@field Y integer (readonly)
---@field Z integer (readonly)
local Vector3int16 = {}

---@class Instance
local Instance = {}

---
---     A wrapper for [`rbx_reflection::ClassDescriptor`] that
---     also provides access to the class descriptor from lua.
---@class DatabaseClass
---@field Name string (readonly)
---@field Superclass string? (readonly)
---@field Properties table<string, DatabaseProperty> (readonly)
---@field DefaultProperties table<string, any> (readonly)
---@field Tags string[] (readonly)
local DatabaseClass = {}

---
---     A wrapper for [`rbx_reflection::EnumDescriptor`] that
---     also provides access to the class descriptor from lua.
---@class DatabaseEnum
---@field Name string (readonly)
---@field Items table<string, integer> (readonly)
local DatabaseEnum = {}

---
---     A wrapper for [`rbx_reflection::PropertyDescriptor`] that
---     also provides access to the property descriptor from lua.
---@class DatabaseProperty
---@field Name string (readonly)
---@field Datatype string (readonly)
---@field Scriptability string (readonly)
---@field Tags string[] (readonly)
local DatabaseProperty = {}

---
---     A wrapper for [`rbx_reflection::ReflectionDatabase`] that
---     also provides access to the reflection database from lua.
---@class Database
---@field Version string (readonly)
local Database = {}

---@return string[]
function Database:GetEnumNames() end

---@return string[]
function Database:GetClassNames() end

---@param name string
---@return DatabaseEnum?
function Database:GetEnum(name) end

---@param name string
---@return DatabaseClass?
function Database:GetClass(name) end

---@param name string
---@return DatabaseEnum?
function Database:FindEnum(name) end

---@param name string
---@return DatabaseClass?
function Database:FindClass(name) end
