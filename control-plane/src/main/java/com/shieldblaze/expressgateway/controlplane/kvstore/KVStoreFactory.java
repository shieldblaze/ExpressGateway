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
package com.shieldblaze.expressgateway.controlplane.kvstore;

import com.orbitz.consul.Consul;
import com.shieldblaze.expressgateway.controlplane.ControlPlaneConfiguration;
import com.shieldblaze.expressgateway.controlplane.kvstore.consul.ConsulKVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.etcd.EtcdKVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.zookeeper.ZooKeeperKVStore;
import io.etcd.jetcd.ByteSequence;
import io.etcd.jetcd.Client;
import io.etcd.jetcd.ClientBuilder;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import lombok.extern.log4j.Log4j2;

import java.io.File;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.Objects;

/**
 * Factory that creates the appropriate {@link KVStore} implementation based on the
 * configured {@link ControlPlaneConfiguration.KvStoreType}.
 *
 * <p>After creating the store, the factory runs startup health checks via
 * {@link BackendHealthChecker} before returning. If the backend is unhealthy,
 * a {@link KVStoreException} is thrown to prevent the control plane from starting
 * in a degraded state.</p>
 */
@Log4j2
public final class KVStoreFactory {

    private KVStoreFactory() {
        // No instantiation -- all static
    }

    /**
     * Creates and returns a connected, health-checked {@link KVStore}.
     *
     * <p>Performs startup safety checks before returning. If the backend
     * is unhealthy after all retries, this method throws.</p>
     *
     * @param type   the KV store backend type
     * @param config the storage configuration with connection and health check settings
     * @return a connected and verified KVStore instance
     * @throws KVStoreException if client creation or health checks fail
     */
    public static KVStore create(ControlPlaneConfiguration.KvStoreType type, StorageConfiguration config)
            throws KVStoreException {
        Objects.requireNonNull(type, "type");
        Objects.requireNonNull(config, "config");

        log.info("Creating KVStore for backend: {}", type);

        KVStore store = switch (type) {
            case ZOOKEEPER -> createZooKeeperStore();
            case ETCD -> createEtcdStore(config);
            case CONSUL -> createConsulStore(config);
        };

        log.info("Running startup health checks for {} backend", type);
        BackendHealthChecker.check(store, config);

        log.info("KVStore created and health-checked successfully: {}", type);
        return store;
    }

    /**
     * Creates a ZooKeeper-backed KV store.
     *
     * <p>ZooKeeper connectivity is managed by the {@link com.shieldblaze.expressgateway.common.zookeeper.Curator}
     * singleton. The {@link ZooKeeperKVStore} uses {@code Curator.getInstance()} internally,
     * so no client needs to be passed in.</p>
     */
    private static KVStore createZooKeeperStore() throws KVStoreException {
        log.info("Creating ZooKeeper KVStore (Curator singleton manages connectivity)");
        return new ZooKeeperKVStore();
    }

    /**
     * Creates an etcd-backed KV store using the jetcd client library.
     *
     * <p>Configures the client with endpoints, TLS, authentication, namespace,
     * and connect timeout from the {@link StorageConfiguration}.</p>
     */
    private static KVStore createEtcdStore(StorageConfiguration config) throws KVStoreException {
        try {
            URI[] endpoints = config.endpoints().stream()
                    .map(URI::create)
                    .toArray(URI[]::new);

            ClientBuilder builder = Client.builder()
                    .endpoints(endpoints)
                    .connectTimeout(Duration.ofMillis(config.connectTimeoutMs()));

            // Authentication
            if (config.username() != null && !config.username().isBlank()) {
                builder.user(ByteSequence.from(config.username(), StandardCharsets.UTF_8));
                if (config.password() != null) {
                    builder.password(ByteSequence.from(config.password(), StandardCharsets.UTF_8));
                }
            }

            // Namespace
            if (config.etcdNamespace() != null && !config.etcdNamespace().isBlank()) {
                builder.namespace(ByteSequence.from(config.etcdNamespace(), StandardCharsets.UTF_8));
            }

            // TLS
            if (config.tlsEnabled()) {
                SslContextBuilder sslBuilder = SslContextBuilder.forClient();

                if (config.tlsCaPath() != null && !config.tlsCaPath().isBlank()) {
                    sslBuilder.trustManager(new File(config.tlsCaPath()));
                }
                if (config.tlsCertPath() != null && !config.tlsCertPath().isBlank()
                        && config.tlsKeyPath() != null && !config.tlsKeyPath().isBlank()) {
                    sslBuilder.keyManager(new File(config.tlsCertPath()), new File(config.tlsKeyPath()));
                }

                SslContext sslContext = sslBuilder.build();
                builder.sslContext(sslContext);
            }

            Client client = builder.build();
            log.info("Created etcd client with endpoints: {}", config.endpoints());
            return new EtcdKVStore(client);

        } catch (Exception e) {
            throw new KVStoreException(KVStoreException.Code.CONNECTION_LOST,
                    "Failed to create etcd client: " + e.getMessage(), e);
        }
    }

    /**
     * Creates a Consul-backed KV store using the orbitz consul-client library.
     *
     * <p>Configures the client with URL, ACL token, authentication, and
     * timeouts from the {@link StorageConfiguration}.</p>
     */
    private static KVStore createConsulStore(StorageConfiguration config) throws KVStoreException {
        try {
            // Consul uses a single URL endpoint
            String endpoint = config.endpoints().get(0);

            Consul.Builder builder = Consul.builder()
                    .withUrl(endpoint)
                    .withConnectTimeoutMillis(config.connectTimeoutMs())
                    .withReadTimeoutMillis(config.operationTimeoutMs())
                    .withPing(false); // We do our own health check via BackendHealthChecker

            // ACL token
            if (config.consulToken() != null && !config.consulToken().isBlank()) {
                builder.withAclToken(config.consulToken());
            }

            // Basic auth
            if (config.username() != null && !config.username().isBlank()
                    && config.password() != null && !config.password().isBlank()) {
                builder.withBasicAuth(config.username(), config.password());
            }

            // TLS
            if (config.tlsEnabled()) {
                builder.withHttps(true);
            }

            Consul consul = builder.build();
            log.info("Created Consul client with endpoint: {}", endpoint);
            return new ConsulKVStore(consul);

        } catch (Exception e) {
            throw new KVStoreException(KVStoreException.Code.CONNECTION_LOST,
                    "Failed to create Consul client: " + e.getMessage(), e);
        }
    }
}
