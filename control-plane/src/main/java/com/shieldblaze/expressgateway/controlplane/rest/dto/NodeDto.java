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
package com.shieldblaze.expressgateway.controlplane.rest.dto;

import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNode;

import java.util.Map;

/**
 * DTO representing a data-plane node's current state and metrics.
 *
 * @param nodeId               the unique node identifier
 * @param clusterId            the cluster this node belongs to
 * @param address              the node's network address
 * @param state                the current lifecycle state
 * @param activeConnections    the number of active connections
 * @param cpuUtilization       CPU utilization (0.0 to 1.0)
 * @param memoryUtilization    memory utilization (0.0 to 1.0)
 * @param appliedConfigVersion the latest config version applied by this node
 * @param connectedAt          ISO-8601 timestamp of when the node connected
 * @param metadata             arbitrary metadata key-value pairs
 */
public record NodeDto(
        String nodeId,
        String clusterId,
        String address,
        String state,
        long activeConnections,
        double cpuUtilization,
        double memoryUtilization,
        long appliedConfigVersion,
        String connectedAt,
        Map<String, String> metadata
) {

    /**
     * Convert a {@link DataPlaneNode} to this DTO.
     */
    public static NodeDto from(DataPlaneNode node) {
        return new NodeDto(
                node.nodeId(),
                node.clusterId(),
                node.address(),
                node.state().name(),
                node.activeConnections(),
                node.cpuUtilization(),
                node.memoryUtilization(),
                node.appliedConfigVersion(),
                node.connectedAt().toString(),
                node.metadata()
        );
    }
}
