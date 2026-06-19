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
    "run_powerflow": lambda p: _run_powerflow(p["assets"]),
    "build_full_network": lambda p: _build_full_network(p["assets"]),
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


def _run_powerflow(assets: dict) -> dict:
    """Run pandapower power flow and return detailed results."""
    net = build_pandapower_net(assets, run_powerflow=True)

    buses = []
    for idx in net.bus.index:
        bus_data = {"id": int(idx)}
        if "vm_pu" in net.res_bus.columns:
            bus_data["vm_pu"] = float(net.res_bus.at[idx, "vm_pu"]) if idx in net.res_bus.index else None
        if "va_degree" in net.res_bus.columns:
            bus_data["va_degree"] = float(net.res_bus.at[idx, "va_degree"]) if idx in net.res_bus.index else None
        buses.append(bus_data)

    lines = []
    for idx in net.line.index:
        line_data = {"id": int(idx)}
        if "p_from_mw" in net.res_line.columns:
            line_data["p_from_mw"] = float(net.res_line.at[idx, "p_from_mw"]) if idx in net.res_line.index else None
        if "q_from_mvar" in net.res_line.columns:
            line_data["q_from_mvar"] = float(net.res_line.at[idx, "q_from_mvar"]) if idx in net.res_line.index else None
        if "p_to_mw" in net.res_line.columns:
            line_data["p_to_mw"] = float(net.res_line.at[idx, "p_to_mw"]) if idx in net.res_line.index else None
        if "q_to_mvar" in net.res_line.columns:
            line_data["q_to_mvar"] = float(net.res_line.at[idx, "q_to_mvar"]) if idx in net.res_line.index else None
        if "pl_mw" in net.res_line.columns:
            line_data["pl_mw"] = float(net.res_line.at[idx, "pl_mw"]) if idx in net.res_line.index else None
        if "ql_mvar" in net.res_line.columns:
            line_data["ql_mvar"] = float(net.res_line.at[idx, "ql_mvar"]) if idx in net.res_line.index else None
        lines.append(line_data)

    trafos = []
    for idx in net.trafo.index:
        trafo_data = {"id": int(idx)}
        if "p_hv_mw" in net.res_trafo.columns:
            trafo_data["p_hv_mw"] = float(net.res_trafo.at[idx, "p_hv_mw"]) if idx in net.res_trafo.index else None
        if "q_hv_mvar" in net.res_trafo.columns:
            trafo_data["q_hv_mvar"] = float(net.res_trafo.at[idx, "q_hv_mvar"]) if idx in net.res_trafo.index else None
        if "p_lv_mw" in net.res_trafo.columns:
            trafo_data["p_lv_mw"] = float(net.res_trafo.at[idx, "p_lv_mw"]) if idx in net.res_trafo.index else None
        if "q_lv_mvar" in net.res_trafo.columns:
            trafo_data["q_lv_mvar"] = float(net.res_trafo.at[idx, "q_lv_mvar"]) if idx in net.res_trafo.index else None
        if "pl_mw" in net.res_trafo.columns:
            trafo_data["pl_mw"] = float(net.res_trafo.at[idx, "pl_mw"]) if idx in net.res_trafo.index else None
        if "ql_mvar" in net.res_trafo.columns:
            trafo_data["ql_mvar"] = float(net.res_trafo.at[idx, "ql_mvar"]) if idx in net.res_trafo.index else None
        trafos.append(trafo_data)

    total_loss_mw = sum(l.get("pl_mw", 0) or 0 for l in lines) + sum(t.get("pl_mw", 0) or 0 for t in trafos)
    total_loss_mvar = sum(l.get("ql_mvar", 0) or 0 for l in lines) + sum(t.get("ql_mvar", 0) or 0 for t in trafos)

    return {
        "converged": bool(net.converged) if hasattr(net, "converged") else False,
        "buses": buses,
        "lines": lines,
        "trafos": trafos,
        "total_loss_mw": total_loss_mw,
        "total_loss_mvar": total_loss_mvar,
    }


def _build_full_network(assets: dict) -> dict:
    """Build pandapower network and return full topology data for Rust side."""
    net = build_pandapower_net(assets, run_powerflow=True)

    # Extract bus data
    buses = []
    for idx in net.bus.index:
        bus = {
            "id": int(idx),
            "name": str(net.bus.at[idx, "name"]) if "name" in net.bus.columns else f"Bus{idx}",
            "vn_kv": float(net.bus.at[idx, "vn_kv"]),
            "type": "PQ",  # default
        }
        # Check if this bus has external grid (slack)
        if "bus" in net.ext_grid.columns and idx in net.ext_grid["bus"].values:
            bus["type"] = "Slack"
        # Check if this bus has generator (PV)
        elif "bus" in net.gen.columns and idx in net.gen["bus"].values:
            bus["type"] = "PV"

        # Generation
        if "bus" in net.gen.columns and idx in net.gen["bus"].values:
            gen_rows = net.gen[net.gen["bus"] == idx]
            bus["p_gen_mw"] = float(gen_rows["p_mw"].sum()) if "p_mw" in gen_rows.columns else 0.0
            bus["q_gen_mvar"] = float(gen_rows["q_mvar"].sum()) if "q_mvar" in gen_rows.columns else 0.0

        # Load
        if "bus" in net.load.columns and idx in net.load["bus"].values:
            load_rows = net.load[net.load["bus"] == idx]
            bus["p_load_mw"] = float(load_rows["p_mw"].sum()) if "p_mw" in load_rows.columns else 0.0
            bus["q_load_mvar"] = float(load_rows["q_mvar"].sum()) if "q_mvar" in load_rows.columns else 0.0

        # Voltage from power flow results
        if hasattr(net, "res_bus") and idx in net.res_bus.index:
            if "vm_pu" in net.res_bus.columns:
                bus["vm_pu"] = float(net.res_bus.at[idx, "vm_pu"])
            if "va_degree" in net.res_bus.columns:
                bus["va_degree"] = float(net.res_bus.at[idx, "va_degree"])

        buses.append(bus)

    # Extract branch data
    branches = []
    # Lines
    for idx in net.line.index:
        line = {
            "id": int(idx),
            "type": "line",
            "from_bus": int(net.line.at[idx, "from_bus"]),
            "to_bus": int(net.line.at[idx, "to_bus"]),
            "length_km": float(net.line.at[idx, "length_km"]) if "length_km" in net.line.columns else 1.0,
            "r_ohm_per_km": float(net.line.at[idx, "r_ohm_per_km"]) if "r_ohm_per_km" in net.line.columns else 0.0,
            "x_ohm_per_km": float(net.line.at[idx, "x_ohm_per_km"]) if "x_ohm_per_km" in net.line.columns else 0.0,
            "c_nf_per_km": float(net.line.at[idx, "c_nf_per_km"]) if "c_nf_per_km" in net.line.columns else 0.0,
            "max_i_ka": float(net.line.at[idx, "max_i_ka"]) if "max_i_ka" in net.line.columns else 1.0,
            "in_service": bool(net.line.at[idx, "in_service"]) if "in_service" in net.line.columns else True,
        }
        branches.append(line)

    # Transformers
    for idx in net.trafo.index:
        trafo = {
            "id": int(idx) + 10000,  # offset to avoid ID collision with lines
            "type": "trafo",
            "from_bus": int(net.trafo.at[idx, "hv_bus"]),
            "to_bus": int(net.trafo.at[idx, "lv_bus"]),
            "sn_mva": float(net.trafo.at[idx, "sn_mva"]) if "sn_mva" in net.trafo.columns else 0.0,
            "vk_percent": float(net.trafo.at[idx, "vk_percent"]) if "vk_percent" in net.trafo.columns else 0.0,
            "vkr_percent": float(net.trafo.at[idx, "vkr_percent"]) if "vkr_percent" in net.trafo.columns else 0.0,
            "tap_pos": int(net.trafo.at[idx, "tap_pos"]) if "tap_pos" in net.trafo.columns else 0,
            "in_service": bool(net.trafo.at[idx, "in_service"]) if "in_service" in net.trafo.columns else True,
        }
        branches.append(trafo)

    # Shunt elements
    shunts = []
    if hasattr(net, 'shunt') and len(net.shunt) > 0:
        for idx in net.shunt.index:
            shunt = {
                "id": int(idx),
                "bus": int(net.shunt.at[idx, "bus"]),
                "q_mvar": float(net.shunt.at[idx, "q_mvar"]) if "q_mvar" in net.shunt.columns else 0.0,
                "p_mw": float(net.shunt.at[idx, "p_mw"]) if "p_mw" in net.shunt.columns else 0.0,
            }
            shunts.append(shunt)

    return {
        "converged": bool(net.converged) if hasattr(net, "converged") else False,
        "base_mva": 1.0,  # pandapower default
        "buses": buses,
        "branches": branches,
        "shunts": shunts,
        "bus_count": len(buses),
        "branch_count": len(branches),
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
            json.dump({"ok": False, "error": f"Unknown command: {command}"}, sys.stdout, ensure_ascii=False)
        else:
            result = COMMAND_MAP[command](params)
            json.dump({"ok": True, "data": result}, sys.stdout, default=_serialize, ensure_ascii=False)
    except Exception as e:
        json.dump({"ok": False, "error": str(e)}, sys.stdout, ensure_ascii=False)
    sys.stdout.write("\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
