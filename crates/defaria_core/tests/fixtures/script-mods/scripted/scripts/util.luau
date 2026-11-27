-- A module: other scripts `require` what it returns.
local util = {}

function util.double(x: number): number
	return x * 2
end

return util
