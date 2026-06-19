#!/bin/bash
# EnerOS 配置注入脚本
# 读取 eneros-machine.yaml，生成 init.toml 和 network.toml，注入到 rootfs 的 /etc/eneros/
# 此文件由 build.sh source

inject_config() {
    local root_mount="$1"
    local machine_config="$2"

    echo "  注入机器配置..."

    local eneros_dir="$root_mount/etc/eneros"
    mkdir -p "$eneros_dir"

    # 复制原始机器配置到 rootfs
    cp "$machine_config" "$eneros_dir/eneros-machine.yaml"

    # 从 YAML 提取 hostname（简单 grep/sed）
    local hostname="eneros-node01"
    if grep -q "hostname:" "$machine_config"; then
        hostname=$(grep "hostname:" "$machine_config" | head -1 | sed 's/.*hostname:[[:space:]]*//; s/#.*//; s/[[:space:]]*$//')
    fi

    # 生成 init.toml
    _generate_init_toml "$eneros_dir/init.toml" "$machine_config" "$hostname"

    # 生成 network.toml
    _generate_network_toml "$eneros_dir/network.toml" "$machine_config" "$hostname"

    echo "  配置注入完成: init.toml, network.toml"
}

# 生成 init.toml（根据 agents 列表）
_generate_init_toml() {
    local out_file="$1"
    local machine_config="$2"
    local hostname="$3"

    cat > "$out_file" << EOF
# EnerOS init 配置
# 由 eneros-imager 从 eneros-machine.yaml 生成

[system]
hostname = "$hostname"

[agents]
# Agent 进程配置（从 eneros-machine.yaml 生成）
EOF

    # 简单解析 YAML 的 agents section，提取 name 和 enabled
    local in_agents=0
    local agent_name=""
    while IFS= read -r line; do
        # 检测进入 agents section
        if echo "$line" | grep -qE "^agents:"; then
            in_agents=1
            continue
        fi
        # 检测离开 agents section（新的顶层 section，非缩进）
        if [ "$in_agents" = "1" ] && echo "$line" | grep -qE "^[a-z_]+:"; then
            in_agents=0
            continue
        fi
        if [ "$in_agents" = "1" ]; then
            if echo "$line" | grep -q "name:"; then
                agent_name=$(echo "$line" | sed 's/.*name:[[:space:]]*//; s/#.*//; s/[[:space:]]*$//')
            elif echo "$line" | grep -q "enabled:" && [ -n "$agent_name" ]; then
                local enabled
                enabled=$(echo "$line" | sed 's/.*enabled:[[:space:]]*//; s/#.*//; s/[[:space:]]*$//')
                echo "# $agent_name: $enabled" >> "$out_file"
                agent_name=""
            fi
        fi
    done < "$machine_config"
}

# 生成 network.toml（根据 network 配置）
_generate_network_toml() {
    local out_file="$1"
    local machine_config="$2"
    local hostname="$3"

    cat > "$out_file" << EOF
# EnerOS 网络配置
# 由 eneros-imager 从 eneros-machine.yaml 生成

hostname = "$hostname"

EOF

    # 简单解析 YAML 的 network.interfaces section
    local in_interfaces=0
    local if_name=""
    local if_method=""
    local if_address=""
    local if_netmask=""
    local if_gateway=""

    while IFS= read -r line; do
        # 检测进入 interfaces 列表
        if echo "$line" | grep -qE "^[[:space:]]*-[[:space:]]*name:"; then
            # 如果已有接口信息，先输出
            if [ -n "$if_name" ]; then
                _write_interface "$out_file" "$if_name" "$if_method" "$if_address" "$if_netmask" "$if_gateway"
            fi
            if_name=$(echo "$line" | sed 's/.*name:[[:space:]]*//; s/#.*//; s/[[:space:]]*$//')
            if_method=""
            if_address=""
            if_netmask=""
            if_gateway=""
            in_interfaces=1
            continue
        fi
        if [ "$in_interfaces" = "1" ]; then
            if echo "$line" | grep -q "method:"; then
                if_method=$(echo "$line" | sed 's/.*method:[[:space:]]*//; s/#.*//; s/[[:space:]]*$//')
            elif echo "$line" | grep -q "address:"; then
                if_address=$(echo "$line" | sed 's/.*address:[[:space:]]*//; s/#.*//; s/[[:space:]]*$//')
            elif echo "$line" | grep -q "netmask:"; then
                if_netmask=$(echo "$line" | sed 's/.*netmask:[[:space:]]*//; s/#.*//; s/[[:space:]]*$//')
            elif echo "$line" | grep -q "gateway:"; then
                if_gateway=$(echo "$line" | sed 's/.*gateway:[[:space:]]*//; s/#.*//; s/[[:space:]]*$//')
            fi
        fi
    done < "$machine_config"

    # 输出最后一个接口
    if [ -n "$if_name" ]; then
        _write_interface "$out_file" "$if_name" "$if_method" "$if_address" "$if_netmask" "$if_gateway"
    fi
}

# 写入单个接口配置到 network.toml
_write_interface() {
    local out_file="$1"
    local name="$2"
    local method="$3"
    local address="$4"
    local netmask="$5"
    local gateway="$6"

    echo "" >> "$out_file"
    echo "[[interfaces]]" >> "$out_file"
    echo "name = \"$name\"" >> "$out_file"
    echo "method = \"${method:-dhcp}\"" >> "$out_file"
    if [ -n "$address" ]; then
        echo "address = \"$address\"" >> "$out_file"
    fi
    if [ -n "$netmask" ]; then
        echo "netmask = \"$netmask\"" >> "$out_file"
    fi
    if [ -n "$gateway" ]; then
        echo "gateway = \"$gateway\"" >> "$out_file"
    fi
}
