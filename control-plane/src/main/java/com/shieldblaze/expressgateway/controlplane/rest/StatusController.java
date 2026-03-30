/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.controlplane.rest;

import com.shieldblaze.expressgateway.controlplane.config.ChangeJournal;
import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNodeState;
import com.shieldblaze.expressgateway.controlplane.registry.NodeRegistry;
import com.shieldblaze.expressgateway.controlplane.rest.dto.ApiResponse;
import lombok.extern.log4j.Log4j2;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.util.LinkedHashMap;
import java.util.Map;

/**
 * REST controller for control plane status and health information.
 *
 * <p>Provides aggregate status of connected data-plane nodes and the current
 * configuration version. Useful for dashboards and operational monitoring.</p>
 */
@RestController
@RequestMapping("/api/v1/controlplane/status")
@Log4j2
public class StatusController {

    private final NodeRegistry nodeRegistry;
    private final ChangeJournal journal;

    public StatusController(NodeRegistry nodeRegistry, ChangeJournal journal) {
        this.nodeRegistry = nodeRegistry;
        this.journal = journal;
    }

    @GetMapping
    public ApiResponse<Map<String, Object>> getStatus() {
        Map<DataPlaneNodeState, Long> nodesByState = nodeRegistry.countByState();

        Map<String, Object> status = new LinkedHashMap<>();
        status.put("totalNodes", nodeRegistry.size());
        status.put("currentConfigVersion", journal.currentRevision());

        Map<String, Long> stateMap = new LinkedHashMap<>();
        for (Map.Entry<DataPlaneNodeState, Long> entry : nodesByState.entrySet()) {
            stateMap.put(entry.getKey().name(), entry.getValue());
        }
        status.put("nodesByState", stateMap);

        return ApiResponse.ok(status);
    }

    @GetMapping("/nodes")
    public ApiResponse<Map<DataPlaneNodeState, Long>> getNodeStatus() {
        return ApiResponse.ok(nodeRegistry.countByState());
    }
}
