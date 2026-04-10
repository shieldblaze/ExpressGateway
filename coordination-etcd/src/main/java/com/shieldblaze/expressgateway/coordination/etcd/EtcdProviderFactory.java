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
package com.shieldblaze.expressgateway.coordination.etcd;

import com.shieldblaze.expressgateway.coordination.CoordinationException;
import com.shieldblaze.expressgateway.coordination.CoordinationProvider;
import com.shieldblaze.expressgateway.coordination.CoordinationProviderFactory;
import io.etcd.jetcd.ByteSequence;
import io.etcd.jetcd.Client;
import io.etcd.jetcd.ClientBuilder;
import lombok.extern.log4j.Log4j2;

import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * Factory for creating {@link EtcdCoordinationProvider} instances from configuration maps.
 *
 * <h2>Supported configuration keys</h2>
 * <table>
 *   <tr><th>Key</th><th>Required</th><th>Default</th><th>Description</th></tr>
 *   <tr><td>{@code endpoints}</td><td>Yes</td><td>-</td><td>Comma-separated etcd endpoints (e.g. "http://host1:2379,http://host2:2379")</td></tr>
 *   <tr><td>{@code username}</td><td>No</td><td>-</td><td>Username for etcd authentication</td></tr>
 *   <tr><td>{@code password}</td><td>No</td><td>-</td><td>Password for etcd authentication</td></tr>
 *   <tr><td>{@code namespace}</td><td>No</td><td>-</td><td>Key namespace prefix (isolates keys within etcd)</td></tr>
 * </table>
 */
@Log4j2
public final class EtcdProviderFactory implements CoordinationProviderFactory {

    @Override
    public String type() {
        return "etcd";
    }

    @Override
    public CoordinationProvider create(Map<String, String> config) throws CoordinationException {
        Objects.requireNonNull(config, "config");

        String endpointsStr = config.get("endpoints");
        if (endpointsStr == null || endpointsStr.isBlank()) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Required configuration key 'endpoints' is missing or blank");
        }

        List<String> endpoints = Arrays.stream(endpointsStr.split(","))
                .map(String::trim)
                .filter(s -> !s.isEmpty())
                .toList();

        if (endpoints.isEmpty()) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "No valid endpoints found in configuration: " + endpointsStr);
        }

        String username = config.get("username");
        String password = config.get("password");
        String namespace = config.get("namespace");

        try {
            ClientBuilder builder = Client.builder()
                    .endpoints(endpoints.toArray(new String[0]));

            if (username != null && !username.isBlank() && password != null && !password.isBlank()) {
                builder.user(ByteSequence.from(username, StandardCharsets.UTF_8))
                        .password(ByteSequence.from(password, StandardCharsets.UTF_8));
            }

            if (namespace != null && !namespace.isBlank()) {
                builder.namespace(ByteSequence.from(namespace, StandardCharsets.UTF_8));
            }

            Client client = builder.build();

            log.info("Connected to etcd at {} (namespace={})", endpoints, namespace);

            // Transfer ownership: the provider will close this client on provider.close()
            return new EtcdCoordinationProvider(client, true);

        } catch (Exception e) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Failed to create etcd client for endpoints: " + endpoints, e);
        }
    }
}
