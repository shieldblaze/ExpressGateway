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

import com.shieldblaze.expressgateway.controlplane.ControlPlaneConfiguration;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.etcd.EtcdKVStore;
import io.etcd.jetcd.Client;
import org.junit.jupiter.api.AfterAll;
import org.testcontainers.containers.GenericContainer;
import org.testcontainers.containers.wait.strategy.Wait;
import org.testcontainers.junit.jupiter.Container;
import org.testcontainers.junit.jupiter.Testcontainers;

import java.util.ArrayList;
import java.util.List;

/**
 * Multi-instance control plane integration test against a real etcd 3.5 instance.
 */
@Testcontainers
class EtcdMultiInstanceTest extends AbstractMultiInstanceTest {

    @Container
    static final GenericContainer<?> ETCD = new GenericContainer<>("quay.io/coreos/etcd:v3.5.17")
            .withExposedPorts(2379)
            .withCommand("etcd",
                    "--listen-client-urls=http://0.0.0.0:2379",
                    "--advertise-client-urls=http://0.0.0.0:2379")
            .waitingFor(Wait.forHttp("/health").forPort(2379).forStatusCode(200));

    private final List<Client> etcdClients = new ArrayList<>();

    @Override
    protected KVStore createKVStore() {
        String endpoint = "http://" + ETCD.getHost() + ":" + ETCD.getMappedPort(2379);
        Client client = Client.builder().endpoints(endpoint).build();
        etcdClients.add(client);
        return new EtcdKVStore(client);
    }

    @Override
    protected ControlPlaneConfiguration.KvStoreType kvStoreType() {
        return ControlPlaneConfiguration.KvStoreType.ETCD;
    }

    @AfterAll
    void closeClients() {
        for (Client client : etcdClients) {
            try {
                client.close();
            } catch (Exception e) {
                // Best effort
            }
        }
        etcdClients.clear();
    }
}
