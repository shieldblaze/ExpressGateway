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

import com.shieldblaze.expressgateway.common.zookeeper.Curator;
import com.shieldblaze.expressgateway.controlplane.ControlPlaneConfiguration;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.zookeeper.ZooKeeperKVStore;
import org.apache.curator.framework.CuratorFramework;
import org.apache.curator.framework.CuratorFrameworkFactory;
import org.apache.curator.retry.RetryNTimes;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.testcontainers.containers.GenericContainer;
import org.testcontainers.containers.wait.strategy.Wait;
import org.testcontainers.junit.jupiter.Container;
import org.testcontainers.junit.jupiter.Testcontainers;

import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

/**
 * Multi-instance control plane integration test against a real ZooKeeper 3.9 instance.
 *
 * <p>Uses reflection to inject a test CuratorFramework into the Curator singleton,
 * following the same pattern as ZooKeeperKVStoreTest. Since ZooKeeperKVStore uses the
 * shared Curator singleton, all instances share the same underlying ZK connection.</p>
 */
@Testcontainers
class ZooKeeperMultiInstanceTest extends AbstractMultiInstanceTest {

    @Container
    static final GenericContainer<?> ZK = new GenericContainer<>("zookeeper:3.9")
            .withExposedPorts(2181)
            .waitingFor(Wait.forListeningPort());

    private CuratorFramework curatorFramework;
    private final List<ZooKeeperKVStore> zkStores = new ArrayList<>();

    @Override
    protected void setUpBackend() throws Exception {
        String connectString = ZK.getHost() + ":" + ZK.getMappedPort(2181);

        curatorFramework = CuratorFrameworkFactory.builder()
                .connectString(connectString)
                .retryPolicy(new RetryNTimes(3, 1000))
                .build();
        curatorFramework.start();
        curatorFramework.blockUntilConnected(30, TimeUnit.SECONDS);

        // Inject the CuratorFramework into the Curator singleton via reflection
        Field instanceField = Curator.class.getDeclaredField("INSTANCE");
        instanceField.setAccessible(true);
        Object curatorInstance = instanceField.get(null);

        Field cfField = Curator.class.getDeclaredField("curatorFramework");
        cfField.setAccessible(true);
        cfField.set(curatorInstance, curatorFramework);

        Field futureField = Curator.class.getDeclaredField("initializationFuture");
        futureField.setAccessible(true);
        CompletableFuture<Boolean> future = (CompletableFuture<Boolean>) futureField.get(curatorInstance);
        if (!future.isDone()) {
            future.complete(true);
        } else {
            futureField.set(curatorInstance, CompletableFuture.completedFuture(true));
        }
    }

    @Override
    protected KVStore createKVStore() {
        ZooKeeperKVStore store = new ZooKeeperKVStore();
        zkStores.add(store);
        return store;
    }

    @Override
    protected ControlPlaneConfiguration.KvStoreType kvStoreType() {
        return ControlPlaneConfiguration.KvStoreType.ZOOKEEPER;
    }

    @AfterAll
    void closeClients() throws Exception {
        for (ZooKeeperKVStore store : zkStores) {
            try {
                store.close();
            } catch (Exception e) {
                // Best effort
            }
        }
        zkStores.clear();
        if (curatorFramework != null) {
            curatorFramework.close();
        }
    }
}
