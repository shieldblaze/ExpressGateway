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

import java.util.Objects;

/**
 * Value object representing a configuration type and its schema version.
 *
 * <p>This is intentionally NOT an enum to allow third-party extensions to define
 * their own config kinds without modifying core code. Well-known kinds are provided
 * as static constants.</p>
 *
 * @param name          The unique name of this config kind (e.g. "cluster", "listener")
 * @param schemaVersion The schema version for this kind, starting at 1
 */
public record ConfigKind(String name, int schemaVersion) {

    public static final ConfigKind CLUSTER = new ConfigKind("cluster", 1);
    public static final ConfigKind LISTENER = new ConfigKind("listener", 1);
    public static final ConfigKind ROUTING_RULE = new ConfigKind("routing-rule", 1);
    public static final ConfigKind LB_STRATEGY = new ConfigKind("lb-strategy", 1);
    public static final ConfigKind HEALTH_CHECK = new ConfigKind("health-check", 1);
    public static final ConfigKind TLS_CERTIFICATE = new ConfigKind("tls-certificate", 1);
    public static final ConfigKind RATE_LIMIT = new ConfigKind("rate-limit", 1);
    public static final ConfigKind SECURITY_POLICY = new ConfigKind("security-policy", 1);
    public static final ConfigKind TRANSPORT = new ConfigKind("transport", 1);
    public static final ConfigKind HTTP = new ConfigKind("http", 1);
    public static final ConfigKind QUIC = new ConfigKind("quic", 1);
    public static final ConfigKind BUFFER = new ConfigKind("buffer", 1);
    public static final ConfigKind NODE_REGISTRY = new ConfigKind("node-registry", 1);
    public static final ConfigKind METRICS_METADATA = new ConfigKind("metrics-metadata", 1);

    public ConfigKind {
        Objects.requireNonNull(name, "name");
        if (name.isBlank()) {
            throw new IllegalArgumentException("name must not be blank");
        }
        if (schemaVersion < 1) {
            throw new IllegalArgumentException("schemaVersion must be >= 1, got: " + schemaVersion);
        }
    }
}
