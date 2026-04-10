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
package com.shieldblaze.expressgateway.coordination.consul;

import com.google.common.net.HostAndPort;
import com.orbitz.consul.Consul;
import com.shieldblaze.expressgateway.coordination.CoordinationException;
import com.shieldblaze.expressgateway.coordination.CoordinationProvider;
import com.shieldblaze.expressgateway.coordination.CoordinationProviderFactory;
import lombok.extern.log4j.Log4j2;

import java.util.Map;
import java.util.Objects;

/**
 * Factory for creating {@link ConsulCoordinationProvider} instances from configuration maps.
 *
 * <h2>Supported configuration keys</h2>
 * <table>
 *   <tr><th>Key</th><th>Required</th><th>Default</th><th>Description</th></tr>
 *   <tr><td>{@code host}</td><td>Yes</td><td>-</td><td>Consul agent host (e.g. "localhost")</td></tr>
 *   <tr><td>{@code port}</td><td>No</td><td>8500</td><td>Consul agent HTTP port</td></tr>
 *   <tr><td>{@code aclToken}</td><td>No</td><td>-</td><td>ACL token for authentication</td></tr>
 *   <tr><td>{@code scheme}</td><td>No</td><td>http</td><td>Connection scheme (http or https)</td></tr>
 * </table>
 */
@Log4j2
public final class ConsulProviderFactory implements CoordinationProviderFactory {

    /** Default Consul HTTP API port. */
    private static final int DEFAULT_PORT = 8500;

    @Override
    public String type() {
        return "consul";
    }

    @Override
    public CoordinationProvider create(Map<String, String> config) throws CoordinationException {
        Objects.requireNonNull(config, "config");

        String host = config.get("host");
        if (host == null || host.isBlank()) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Required configuration key 'host' is missing or blank");
        }

        int port = getInt(config, "port", DEFAULT_PORT);
        String aclToken = config.get("aclToken");
        String scheme = config.getOrDefault("scheme", "http");

        try {
            Consul.Builder builder = Consul.builder()
                    .withHostAndPort(HostAndPort.fromParts(host, port));

            if ("https".equalsIgnoreCase(scheme)) {
                builder.withHttps(true);
            }

            if (aclToken != null && !aclToken.isBlank()) {
                builder.withAclToken(aclToken);
            }

            Consul consul = builder.build();

            // Verify connectivity with a ping
            try {
                consul.agentClient().ping();
            } catch (Exception e) {
                consul.destroy();
                throw new CoordinationException(CoordinationException.Code.CONNECTION_LOST,
                        "Failed to connect to Consul at " + scheme + "://" + host + ":" + port, e);
            }

            log.info("Connected to Consul at {}://{}:{} (acl={})",
                    scheme, host, port, aclToken != null && !aclToken.isBlank() ? "enabled" : "disabled");

            // Transfer ownership: the provider will destroy this client on provider.close()
            return new ConsulCoordinationProvider(consul, true);
        } catch (CoordinationException e) {
            throw e;
        } catch (Exception e) {
            throw new CoordinationException(CoordinationException.Code.CONNECTION_LOST,
                    "Failed to create Consul client for " + scheme + "://" + host + ":" + port, e);
        }
    }

    /**
     * Extracts an integer from the config map with a default fallback.
     */
    private static int getInt(Map<String, String> config, String key, int defaultValue) {
        String value = config.get(key);
        if (value == null || value.isBlank()) {
            return defaultValue;
        }
        try {
            return Integer.parseInt(value.trim());
        } catch (NumberFormatException e) {
            log.warn("Invalid integer value for config key '{}': '{}', using default {}",
                    key, value, defaultValue);
            return defaultValue;
        }
    }
}
