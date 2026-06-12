"""EnerOS Bridge - Python side.
Called by Rust via subprocess, reads JSON from stdin, writes JSON to stdout.
"""
import json
import sys
from cnpower.equipment import (
    get_all_transformers,
    get_all_cables,
    get_all_overhead_lines,
    get_all_switchgear,
    get_all_reactive_compensation,
    get_all_protection,
    get_all_instrument_transformers,
    get_all_surge_arresters,
)
from cnpower.equipment.new_energy import (
    get_all_photovoltaic,
    get_all_wind_turbines,
    get_all_energy_storage,
    get_all_ev_chargers,
)
from cnpower.standards import get_all_standards
from cnpower.topology import get_all_connection_modes
from cnpower.validation import get_all_validation_rules
from cnpower.engineering import (
    normalize_equipment,
    check_equipment_compliance,
)
from cnpower.engineering.network_builder import build_pandapower_net
from cnpower.pandapower_integration import add_chinese_std_types

COMMAND_MAP = {
    "list_transformers": lambda _: _flatten_equipment(get_all_transformers()),
    "list_cables": lambda _: _flatten_equipment(get_all_cables()),
    "list_overhead_lines": lambda _: _flatten_equipment(get_all_overhead_lines()),
    "list_switchgear": lambda _: _flatten_equipment(get_all_switchgear()),
    "list_reactive_compensation": lambda _: _flatten_equipment(get_all_reactive_compensation()),
    "list_protection": lambda _: _flatten_equipment(get_all_protection()),
    "list_instrument_transformers": lambda _: _flatten_equipment(get_all_instrument_transformers()),
    "list_surge_arresters": lambda _: _flatten_equipment(get_all_surge_arresters()),
    "list_photovoltaic": lambda _: _flatten_equipment(get_all_photovoltaic()),
    "list_wind_turbines": lambda _: _flatten_equipment(get_all_wind_turbines()),
    "list_energy_storage": lambda _: _flatten_equipment(get_all_energy_storage()),
    "list_ev_chargers": lambda _: _flatten_equipment(get_all_ev_chargers()),
    "list_standards": lambda _: get_all_standards(),
    "list_connection_modes": lambda _: get_all_connection_modes(),
    "list_validation_rules": lambda _: get_all_validation_rules(),
    "get_transformer": lambda p: _get_equipment(get_all_transformers(), p["category"], p["model"]),
    "get_cable": lambda p: _get_equipment(get_all_cables(), p["category"], p["model"]),
    "get_overhead_line": lambda p: _get_equipment(get_all_overhead_lines(), p["category"], p["model"]),
    "get_switchgear": lambda p: _get_equipment(get_all_switchgear(), p["category"], p["model"]),
    "normalize_equipment": lambda p: normalize_equipment(p["equipment_type"], p["params"]),
    "check_compliance": lambda p: check_equipment_compliance(p["equipment_type"], p["spec"], p["operating"]),
    "build_network": lambda p: _build_network(p["assets"], p.get("run_powerflow", False)),
}


def _flatten_equipment(grouped: dict) -> list:
    result = []
    for category, models in grouped.items():
        if isinstance(models, dict):
            for model_name, data in models.items():
                entry = dict(data) if isinstance(data, dict) else {"value": data}
                entry["_category"] = category
                entry["_model"] = model_name
                result.append(entry)
        else:
            result.append({"_category": category, "value": models})
    return result


def _get_equipment(grouped: dict, category: str, model: str) -> dict:
    cat = grouped.get(category, {})
    if isinstance(cat, dict):
        return cat.get(model, {"error": f"Model '{model}' not found in '{category}'"})
    return {"error": f"Category '{category}' not found"}


def _build_network(assets: dict, run_powerflow: bool) -> dict:
    net = build_pandapower_net(assets, run_powerflow=run_powerflow)
    return {
        "converged": bool(net.converged) if hasattr(net, "converged") else None,
        "bus_count": len(net.bus),
        "line_count": len(net.line),
        "trafo_count": len(net.trafo),
        "load_count": len(net.load),
        "ext_grid_count": len(net.ext_grid),
    }


def _serialize(obj):
    if hasattr(obj, "isoformat"):
        return obj.isoformat()
    if isinstance(obj, set):
        return list(obj)
    raise TypeError(f"Object of type {type(obj)} is not JSON serializable")


def main():
    try:
        raw = sys.stdin.read()
        request = json.loads(raw)
        command = request.get("command", "")
        params = request.get("params", {})

        if command not in COMMAND_MAP:
            result = {"error": f"Unknown command: {command}"}
        else:
            result = COMMAND_MAP[command](params)

        json.dump({"ok": True, "data": result}, sys.stdout, default=_serialize, ensure_ascii=False)
    except Exception as e:
        json.dump({"ok": False, "error": str(e)}, sys.stdout, ensure_ascii=False)
    sys.stdout.write("\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
