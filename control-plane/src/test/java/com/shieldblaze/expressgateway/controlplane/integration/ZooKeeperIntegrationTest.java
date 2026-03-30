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
package com.shieldblaze.expressgateway.controlplane.integration;

import com.shieldblaze.expressgateway.common.zookeeper.Curator;
import com.shieldblaze.expressgateway.controlplane.ControlPlaneConfiguration;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.zookeeper.ZooKeeperKVStore;
import org.apache.curator.framework.CuratorFramework;
import org.apache.curator.framework.CuratorFrameworkFactory;
import org.apache.curator.retry.RetryNTimes;
import org.junit.jupiter.api.AfterAll;
import org.testcontainers.containers.GenericContainer;
import org.testcontainers.containers.wait.strategy.Wait;
import org.testcontainers.junit.jupiter.Container;
import org.testcontainers.junit.jupiter.Testcontainers;

import java.lang.reflect.Field;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

/**
 * End-to-end control plane integration test against a real ZooKeeper 3.9 instance.
 *
 * <p>Uses reflection to inject a test CuratorFramework into the {@link Curator} singleton,
 * following the same pattern as ZooKeeperKVStoreTest.</p>
 */
@Testcontainers
class ZooKeeperIntegrationTest extends AbstractControlPlaneIntegrationTest {

    @Container
    static final GenericContainer<?> ZK = new GenericContainer<>("zookeeper:3.9")
            .withExposedPorts(2181)
            .waitingFor(Wait.forListeningPort());

    private static CuratorFramework curatorFramework;
    private static ZooKeeperKVStore kvStore;

    @Override
    protected KVStore createKVStore() throws Exception {
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

        kvStore = new ZooKeeperKVStore();
        return kvStore;
    }

    @Override
    protected ControlPlaneConfiguration.KvStoreType kvStoreType() {
        return ControlPlaneConfiguration.KvStoreType.ZOOKEEPER;
    }

    @AfterAll
    void closeKVStore() throws Exception {
        if (kvStore != null) {
            kvStore.close();
        }
        if (curatorFramework != null) {
            curatorFramework.close();
        }
    }
}
