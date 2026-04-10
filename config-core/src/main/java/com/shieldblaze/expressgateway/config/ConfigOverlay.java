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
package com.shieldblaze.expressgateway.config;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import com.fasterxml.jackson.databind.ObjectMapper;

import java.util.LinkedHashMap;
import java.util.Map;
import java.util.function.Function;

/**
 * Internal helper that applies environment variable and system property overlays
 * on top of a Jackson {@link ObjectNode} tree before binding to {@link GatewayConfig}.
 *
 * <p>Resolution order: environment variables (prefix {@code EG_}) are applied first,
 * then system properties (prefix {@code expressgateway.}), then explicit overrides.
 * Each layer overwrites the previous, giving system properties precedence over env vars.</p>
 */
final class ConfigOverlay {

    /**
     * Mapping from environment variable name to the JSON pointer path in the config tree.
     * The values use "/" as a path separator for nested fields.
     */
    private static final Map<String, String> ENV_VAR_MAPPINGS = new LinkedHashMap<>();

    /**
     * Mapping from system property name to the JSON pointer path in the config tree.
     */
    private static final Map<String, String> SYS_PROP_MAPPINGS = new LinkedHashMap<>();

    static {
        // Top-level
        ENV_VAR_MAPPINGS.put("EG_CLUSTER_ID", "/clusterId");
        ENV_VAR_MAPPINGS.put("EG_RUNNING_MODE", "/runningMode");
        ENV_VAR_MAPPINGS.put("EG_ENVIRONMENT", "/environment");

        // REST API
        ENV_VAR_MAPPINGS.put("EG_REST_API_BIND_ADDRESS", "/restApi/bindAddress");
        ENV_VAR_MAPPINGS.put("EG_REST_API_PORT", "/restApi/port");
        ENV_VAR_MAPPINGS.put("EG_REST_API_TLS_ENABLED", "/restApi/tlsEnabled");

        // Coordination
        ENV_VAR_MAPPINGS.put("EG_COORDINATION_BACKEND", "/coordination/backend");
        ENV_VAR_MAPPINGS.put("EG_COORDINATION_ENDPOINTS", "/coordination/endpoints");
        ENV_VAR_MAPPINGS.put("EG_COORDINATION_SESSION_TIMEOUT_MS", "/coordination/sessionTimeoutMs");
        ENV_VAR_MAPPINGS.put("EG_COORDINATION_CONNECTION_TIMEOUT_MS", "/coordination/connectionTimeoutMs");
        ENV_VAR_MAPPINGS.put("EG_COORDINATION_USERNAME", "/coordination/username");
        ENV_VAR_MAPPINGS.put("EG_COORDINATION_NAMESPACE", "/coordination/namespace");

        // Service Discovery
        ENV_VAR_MAPPINGS.put("EG_SERVICE_DISCOVERY_URI", "/serviceDiscovery/uri");
        ENV_VAR_MAPPINGS.put("EG_SERVICE_DISCOVERY_TRUST_ALL_CERTS", "/serviceDiscovery/trustAllCerts");
        ENV_VAR_MAPPINGS.put("EG_SERVICE_DISCOVERY_HOSTNAME_VERIFICATION", "/serviceDiscovery/hostnameVerification");

        // Control Plane
        ENV_VAR_MAPPINGS.put("EG_CONTROL_PLANE_ENABLED", "/controlPlane/enabled");
        ENV_VAR_MAPPINGS.put("EG_CONTROL_PLANE_GRPC_BIND_ADDRESS", "/controlPlane/grpcBindAddress");
        ENV_VAR_MAPPINGS.put("EG_CONTROL_PLANE_GRPC_PORT", "/controlPlane/grpcPort");
        ENV_VAR_MAPPINGS.put("EG_CONTROL_PLANE_HEARTBEAT_INTERVAL_MS", "/controlPlane/heartbeatIntervalMs");
        ENV_VAR_MAPPINGS.put("EG_CONTROL_PLANE_HEARTBEAT_MISS_THRESHOLD", "/controlPlane/heartbeatMissThreshold");
        ENV_VAR_MAPPINGS.put("EG_CONTROL_PLANE_MAX_NODES", "/controlPlane/maxNodes");
        ENV_VAR_MAPPINGS.put("EG_CONTROL_PLANE_ADDRESS", "/controlPlane/controlPlaneAddress");
        ENV_VAR_MAPPINGS.put("EG_CONTROL_PLANE_PORT", "/controlPlane/controlPlanePort");

        // System property mappings (expressgateway.xxx -> JSON pointer)
        SYS_PROP_MAPPINGS.put("expressgateway.cluster.id", "/clusterId");
        SYS_PROP_MAPPINGS.put("expressgateway.running.mode", "/runningMode");
        SYS_PROP_MAPPINGS.put("expressgateway.environment", "/environment");
        SYS_PROP_MAPPINGS.put("expressgateway.rest.api.bind.address", "/restApi/bindAddress");
        SYS_PROP_MAPPINGS.put("expressgateway.rest.api.port", "/restApi/port");
        SYS_PROP_MAPPINGS.put("expressgateway.rest.api.tls.enabled", "/restApi/tlsEnabled");
        SYS_PROP_MAPPINGS.put("expressgateway.coordination.backend", "/coordination/backend");
        SYS_PROP_MAPPINGS.put("expressgateway.coordination.endpoints", "/coordination/endpoints");
        SYS_PROP_MAPPINGS.put("expressgateway.coordination.session.timeout.ms", "/coordination/sessionTimeoutMs");
        SYS_PROP_MAPPINGS.put("expressgateway.coordination.connection.timeout.ms", "/coordination/connectionTimeoutMs");
        SYS_PROP_MAPPINGS.put("expressgateway.coordination.username", "/coordination/username");
        SYS_PROP_MAPPINGS.put("expressgateway.coordination.namespace", "/coordination/namespace");
        SYS_PROP_MAPPINGS.put("expressgateway.service.discovery.uri", "/serviceDiscovery/uri");
        SYS_PROP_MAPPINGS.put("expressgateway.control.plane.enabled", "/controlPlane/enabled");
        SYS_PROP_MAPPINGS.put("expressgateway.control.plane.grpc.bind.address", "/controlPlane/grpcBindAddress");
        SYS_PROP_MAPPINGS.put("expressgateway.control.plane.grpc.port", "/controlPlane/grpcPort");
    }

    private ConfigOverlay() {
        // utility class
    }

    /**
     * Applies environment variables and system properties on top of the given tree.
     *
     * @param root   the Jackson tree loaded from the config file (mutated in place)
     * @param mapper the ObjectMapper to use for node creation
     * @param envLookup function to resolve environment variables (allows testing)
     * @param sysLookup function to resolve system properties (allows testing)
     */
    static void apply(ObjectNode root, ObjectMapper mapper,
                      Function<String, String> envLookup,
                      Function<String, String> sysLookup) {
        // Layer 1: environment variables
        for (Map.Entry<String, String> entry : ENV_VAR_MAPPINGS.entrySet()) {
            String value = envLookup.apply(entry.getKey());
            if (value != null && !value.isBlank()) {
                setValueAtPath(root, mapper, entry.getValue(), value, entry.getKey());
            }
        }

        // Layer 2: system properties (higher precedence)
        for (Map.Entry<String, String> entry : SYS_PROP_MAPPINGS.entrySet()) {
            String value = sysLookup.apply(entry.getKey());
            if (value != null && !value.isBlank()) {
                setValueAtPath(root, mapper, entry.getValue(), value, entry.getKey());
            }
        }
    }

    /**
     * Applies explicit overrides from a map (highest precedence layer).
     *
     * @param root      the Jackson tree (mutated in place)
     * @param mapper    the ObjectMapper
     * @param overrides key-value pairs where keys are JSON pointer paths (e.g. "/clusterId")
     *                  or flat property names (e.g. "clusterId", "restApi.port")
     */
    static void applyOverrides(ObjectNode root, ObjectMapper mapper, Map<String, String> overrides) {
        for (Map.Entry<String, String> entry : overrides.entrySet()) {
            String path = normalizePath(entry.getKey());
            setValueAtPath(root, mapper, path, entry.getValue(), entry.getKey());
        }
    }

    /**
     * Normalizes a property-style key (e.g. "restApi.port") to a JSON pointer (e.g. "/restApi/port").
     */
    private static String normalizePath(String key) {
        if (key.startsWith("/")) {
            return key;
        }
        return "/" + key.replace('.', '/');
    }

    /**
     * Sets a value at the specified JSON pointer path within the tree.
     * Creates intermediate object nodes as needed.
     *
     * @param root      the root object node
     * @param mapper    the ObjectMapper for node creation
     * @param jsonPath  the JSON pointer path (e.g. "/restApi/port")
     * @param rawValue  the string value to set
     * @param sourceKey the original env var or property name (for context in diagnostics)
     */
    private static void setValueAtPath(ObjectNode root, ObjectMapper mapper,
                                       String jsonPath, String rawValue, String sourceKey) {
        String[] segments = jsonPath.split("/");
        // Skip empty first segment from leading "/"
        ObjectNode current = root;
        for (int i = 1; i < segments.length - 1; i++) {
            String segment = segments[i];
            JsonNode child = current.get(segment);
            if (child == null || !child.isObject()) {
                ObjectNode newNode = mapper.createObjectNode();
                current.set(segment, newNode);
                current = newNode;
            } else {
                current = (ObjectNode) child;
            }
        }

        String fieldName = segments[segments.length - 1];
        String path = jsonPath;

        // Determine how to coerce the value based on the path
        if (isCommaSeparatedField(path)) {
            ArrayNode arrayNode = mapper.createArrayNode();
            for (String element : rawValue.split(",")) {
                String trimmed = element.trim();
                if (!trimmed.isEmpty()) {
                    arrayNode.add(trimmed);
                }
            }
            current.set(fieldName, arrayNode);
        } else if (isBooleanField(path)) {
            current.put(fieldName, Boolean.parseBoolean(rawValue));
        } else if (isIntegerField(path)) {
            try {
                current.put(fieldName, Integer.parseInt(rawValue.trim()));
            } catch (NumberFormatException _) {
                // Leave as string — Jackson deserialization will report the error
                current.put(fieldName, rawValue);
            }
        } else if (isLongField(path)) {
            try {
                current.put(fieldName, Long.parseLong(rawValue.trim()));
            } catch (NumberFormatException _) {
                current.put(fieldName, rawValue);
            }
        } else {
            current.put(fieldName, rawValue);
        }
    }

    private static boolean isCommaSeparatedField(String path) {
        return "/coordination/endpoints".equals(path);
    }

    private static boolean isBooleanField(String path) {
        return switch (path) {
            case "/restApi/tlsEnabled",
                 "/serviceDiscovery/trustAllCerts",
                 "/serviceDiscovery/hostnameVerification",
                 "/controlPlane/enabled" -> true;
            default -> false;
        };
    }

    private static boolean isIntegerField(String path) {
        return switch (path) {
            case "/restApi/port",
                 "/coordination/sessionTimeoutMs",
                 "/coordination/connectionTimeoutMs",
                 "/controlPlane/grpcPort",
                 "/controlPlane/heartbeatMissThreshold",
                 "/controlPlane/maxNodes",
                 "/controlPlane/controlPlanePort" -> true;
            default -> false;
        };
    }

    private static boolean isLongField(String path) {
        return "/controlPlane/heartbeatIntervalMs".equals(path);
    }
}
