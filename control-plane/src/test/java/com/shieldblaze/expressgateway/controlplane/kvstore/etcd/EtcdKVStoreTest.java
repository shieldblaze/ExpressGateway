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
package com.shieldblaze.expressgateway.controlplane.kvstore.etcd;

import com.shieldblaze.expressgateway.controlplane.kvstore.AbstractKVStoreTest;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import io.etcd.jetcd.Client;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.testcontainers.containers.GenericContainer;
import org.testcontainers.containers.wait.strategy.Wait;
import org.testcontainers.junit.jupiter.Container;
import org.testcontainers.junit.jupiter.Testcontainers;

import java.io.IOException;

/**
 * Testcontainers-based integration test for {@link EtcdKVStore}.
 *
 * <p>Runs a real etcd 3.5 instance in a Docker container and executes the full
 * {@link AbstractKVStoreTest} suite against it. The container is started once
 * per class (shared across all test methods).</p>
 */
@Testcontainers
class EtcdKVStoreTest extends AbstractKVStoreTest {

    @Container
    static final GenericContainer<?> ETCD = new GenericContainer<>("quay.io/coreos/etcd:v3.5.17")
            .withExposedPorts(2379)
            .withCommand("etcd",
                    "--listen-client-urls=http://0.0.0.0:2379",
                    "--advertise-client-urls=http://0.0.0.0:2379")
            .waitingFor(Wait.forHttp("/health").forPort(2379).forStatusCode(200));

    private static EtcdKVStore store;
    private static Client client;

    @BeforeAll
    static void setUp() {
        String endpoint = "http://" + ETCD.getHost() + ":" + ETCD.getMappedPort(2379);
        client = Client.builder()
                .endpoints(endpoint)
                .build();
        store = new EtcdKVStore(client);
    }

    @AfterAll
    static void tearDown() throws IOException {
        if (store != null) {
            store.close();
        }
    }

    @Override
    protected KVStore kvStore() {
        return store;
    }
}
