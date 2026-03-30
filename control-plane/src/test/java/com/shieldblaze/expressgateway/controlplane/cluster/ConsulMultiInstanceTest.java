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
package com.shieldblaze.expressgateway.controlplane.cluster;

import com.orbitz.consul.Consul;
import com.shieldblaze.expressgateway.controlplane.ControlPlaneConfiguration;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.consul.ConsulKVStore;
import org.junit.jupiter.api.AfterAll;
import org.testcontainers.containers.GenericContainer;
import org.testcontainers.containers.wait.strategy.Wait;
import org.testcontainers.junit.jupiter.Container;
import org.testcontainers.junit.jupiter.Testcontainers;

import java.util.ArrayList;
import java.util.List;

/**
 * Multi-instance control plane integration test against a real Consul 1.18 instance.
 */
@Testcontainers
class ConsulMultiInstanceTest extends AbstractMultiInstanceTest {

    @Container
    static final GenericContainer<?> CONSUL = new GenericContainer<>("hashicorp/consul:1.18")
            .withExposedPorts(8500)
            .withCommand("agent", "-dev", "-client=0.0.0.0")
            .waitingFor(Wait.forHttp("/v1/status/leader").forPort(8500).forStatusCode(200));

    private final List<ConsulKVStore> consulStores = new ArrayList<>();

    @Override
    protected KVStore createKVStore() {
        String url = "http://" + CONSUL.getHost() + ":" + CONSUL.getMappedPort(8500);
        Consul consul = Consul.builder()
                .withUrl(url)
                .withReadTimeoutMillis(60_000L)
                .build();
        ConsulKVStore store = new ConsulKVStore(consul);
        consulStores.add(store);
        return store;
    }

    @Override
    protected ControlPlaneConfiguration.KvStoreType kvStoreType() {
        return ControlPlaneConfiguration.KvStoreType.CONSUL;
    }

    @AfterAll
    void closeClients() {
        for (ConsulKVStore store : consulStores) {
            try {
                store.close();
            } catch (Exception e) {
                // Best effort
            }
        }
        consulStores.clear();
    }
}
