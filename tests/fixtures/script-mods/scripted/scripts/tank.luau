-- Engine handles come in as userdata that works only during the call.
local kept

function on_fill(tank, amount: number): number
	tank:fill(amount)
	return tank:amount()
end

function on_keep(tank)
	kept = tank
end

function on_use_kept(): number
	return kept:amount()
end
