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

import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNode;
import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNodeState;
import com.shieldblaze.expressgateway.controlplane.registry.NodeRegistry;
import com.shieldblaze.expressgateway.controlplane.rest.dto.ApiResponse;
import com.shieldblaze.expressgateway.controlplane.rest.dto.NodeDto;
import lombok.extern.log4j.Log4j2;

import static com.shieldblaze.expressgateway.common.utils.LogSanitizer.sanitize;

import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.util.List;
import java.util.Optional;

/**
 * REST controller for data-plane node management.
 *
 * <p>Exposes read-only views of connected nodes plus operational actions
 * (drain, undrain, force-remove). Nodes are registered and deregistered
 * via gRPC; this controller provides the administrative REST interface.</p>
 */
@RestController
@RequestMapping("/api/v1/controlplane/nodes")
@Log4j2
public class NodeController {

    private final NodeRegistry nodeRegistry;

    public NodeController(NodeRegistry nodeRegistry) {
        this.nodeRegistry = nodeRegistry;
    }

    @GetMapping
    public ApiResponse<List<NodeDto>> listNodes() {
        List<NodeDto> nodes = nodeRegistry.allNodes().stream()
                .map(NodeDto::from)
                .toList();
        return ApiResponse.ok(nodes);
    }

    @GetMapping("/{nodeId}")
    public ApiResponse<NodeDto> getNode(@PathVariable String nodeId) {
        Optional<DataPlaneNode> node = nodeRegistry.get(nodeId);
        if (node.isEmpty()) {
            return ApiResponse.error("Node not found: " + nodeId);
        }
        return ApiResponse.ok(NodeDto.from(node.get()));
    }

    @PostMapping("/{nodeId}/drain")
    public ApiResponse<String> drainNode(@PathVariable String nodeId) {
        Optional<DataPlaneNode> node = nodeRegistry.get(nodeId);
        if (node.isEmpty()) {
            return ApiResponse.error("Node not found: " + nodeId);
        }

        DataPlaneNode n = node.get();
        if (n.state() == DataPlaneNodeState.DISCONNECTED) {
            return ApiResponse.error("Cannot drain a disconnected node: " + nodeId);
        }
        if (n.state() == DataPlaneNodeState.DRAINING) {
            return ApiResponse.ok("Node is already draining", nodeId);
        }

        n.markDraining();
        log.info("Drained node via REST: {}", sanitize(nodeId));
        return ApiResponse.ok("Node marked as draining", nodeId);
    }

    @PostMapping("/{nodeId}/undrain")
    public ApiResponse<String> undrainNode(@PathVariable String nodeId) {
        Optional<DataPlaneNode> node = nodeRegistry.get(nodeId);
        if (node.isEmpty()) {
            return ApiResponse.error("Node not found: " + nodeId);
        }

        DataPlaneNode n = node.get();
        if (n.state() != DataPlaneNodeState.DRAINING) {
            return ApiResponse.error("Node is not in DRAINING state: " + nodeId + " (current: " + n.state() + ")");
        }

        n.markHealthy();
        log.info("Undrained node via REST: {}", sanitize(nodeId));
        return ApiResponse.ok("Node marked as healthy", nodeId);
    }

    @DeleteMapping("/{nodeId}")
    public ApiResponse<String> removeNode(@PathVariable String nodeId) {
        DataPlaneNode removed = nodeRegistry.deregister(nodeId);
        if (removed == null) {
            return ApiResponse.error("Node not found: " + nodeId);
        }

        log.info("Force-removed node via REST: {}", sanitize(nodeId));
        return ApiResponse.ok("Node removed", nodeId);
    }
}
