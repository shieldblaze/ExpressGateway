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
package com.shieldblaze.expressgateway.controlplane.config;

import com.fasterxml.jackson.annotation.JsonSubTypes;
import com.fasterxml.jackson.annotation.JsonTypeInfo;

import java.util.Objects;

/**
 * Sealed interface defining the scope at which a configuration resource applies.
 *
 * <p>Scopes form a hierarchy: {@link Global} applies everywhere, {@link ClusterScoped}
 * applies to a specific cluster, and {@link NodeScoped} applies to a single node.
 * Each scope produces a unique qualifier string for use in resource identification
 * and KV store key construction.</p>
 */
@JsonTypeInfo(use = JsonTypeInfo.Id.NAME, property = "@type")
@JsonSubTypes({
        @JsonSubTypes.Type(value = ConfigScope.Global.class, name = "global"),
        @JsonSubTypes.Type(value = ConfigScope.ClusterScoped.class, name = "cluster"),
        @JsonSubTypes.Type(value = ConfigScope.NodeScoped.class, name = "node")
})
public sealed interface ConfigScope permits ConfigScope.Global, ConfigScope.ClusterScoped, ConfigScope.NodeScoped {

    /**
     * Global scope: configuration applies across all clusters and nodes.
     */
    record Global() implements ConfigScope {
        @Override
        public String qualifier() {
            return "global";
        }
    }

    /**
     * Cluster-scoped: configuration applies to a specific cluster.
     *
     * @param clusterId The unique identifier of the cluster
     */
    record ClusterScoped(String clusterId) implements ConfigScope {
        public ClusterScoped {
            Objects.requireNonNull(clusterId, "clusterId");
            if (clusterId.isBlank()) {
                throw new IllegalArgumentException("clusterId must not be blank");
            }
        }

        @Override
        public String qualifier() {
            return "cluster:" + clusterId;
        }
    }

    /**
     * Node-scoped: configuration applies to a specific node instance.
     *
     * @param nodeId The unique identifier of the node
     */
    record NodeScoped(String nodeId) implements ConfigScope {
        public NodeScoped {
            Objects.requireNonNull(nodeId, "nodeId");
            if (nodeId.isBlank()) {
                throw new IllegalArgumentException("nodeId must not be blank");
            }
        }

        @Override
        public String qualifier() {
            return "node:" + nodeId;
        }
    }

    /**
     * Returns a unique qualifier string for this scope.
     * Used as a component in {@link ConfigResourceId} paths.
     *
     * @return The qualifier string (e.g. "global", "cluster:my-cluster", "node:node-1")
     */
    String qualifier();

    /**
     * Parse a qualifier string back into a {@link ConfigScope} instance.
     *
     * @param qualifier The qualifier string to parse
     * @return The corresponding {@link ConfigScope}
     * @throws IllegalArgumentException if the qualifier format is unrecognized
     */
    static ConfigScope fromQualifier(String qualifier) {
        Objects.requireNonNull(qualifier, "qualifier");
        if ("global".equals(qualifier)) {
            return new Global();
        }
        if (qualifier.startsWith("cluster:")) {
            String clusterId = qualifier.substring("cluster:".length());
            if (clusterId.isBlank()) {
                throw new IllegalArgumentException("cluster qualifier missing clusterId: " + qualifier);
            }
            return new ClusterScoped(clusterId);
        }
        if (qualifier.startsWith("node:")) {
            String nodeId = qualifier.substring("node:".length());
            if (nodeId.isBlank()) {
                throw new IllegalArgumentException("node qualifier missing nodeId: " + qualifier);
            }
            return new NodeScoped(nodeId);
        }
        throw new IllegalArgumentException("Unrecognized qualifier format: " + qualifier);
    }
}
