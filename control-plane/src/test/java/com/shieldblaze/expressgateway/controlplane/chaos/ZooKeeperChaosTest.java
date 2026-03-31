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
package com.shieldblaze.expressgateway.controlplane.chaos;

import com.shieldblaze.expressgateway.common.zookeeper.Curator;
import com.shieldblaze.expressgateway.controlplane.ControlPlaneConfiguration;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.zookeeper.ZooKeeperKVStore;
import org.apache.curator.framework.CuratorFramework;
import org.apache.curator.framework.CuratorFrameworkFactory;
import org.apache.curator.retry.RetryNTimes;
import org.testcontainers.containers.GenericContainer;
import org.testcontainers.containers.wait.strategy.Wait;
import org.testcontainers.junit.jupiter.Container;
import org.testcontainers.junit.jupiter.Testcontainers;

import java.lang.reflect.Field;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

/**
 * Backend chaos tests against a real ZooKeeper 3.9 instance.
 */
@Testcontainers
class ZooKeeperChaosTest extends AbstractBackendChaosTest {

    @Container
    static final GenericContainer<?> ZK = new GenericContainer<>("zookeeper:3.9")
            .withExposedPorts(2181)
            .waitingFor(Wait.forListeningPort());

    @Override
    protected KVStore createKVStore() throws Exception {
        String connectString = ZK.getHost() + ":" + ZK.getMappedPort(2181);

        CuratorFramework curator = CuratorFrameworkFactory.builder()
                .connectString(connectString)
                .retryPolicy(new RetryNTimes(3, 1000))
                .build();
        curator.start();
        curator.blockUntilConnected(30, TimeUnit.SECONDS);

        // Inject into Curator singleton
        Field instanceField = Curator.class.getDeclaredField("INSTANCE");
        instanceField.setAccessible(true);
        Object curatorInstance = instanceField.get(null);

        Field cfField = Curator.class.getDeclaredField("curatorFramework");
        cfField.setAccessible(true);
        cfField.set(curatorInstance, curator);

        Field futureField = Curator.class.getDeclaredField("initializationFuture");
        futureField.setAccessible(true);
        CompletableFuture<Boolean> future = (CompletableFuture<Boolean>) futureField.get(curatorInstance);
        if (!future.isDone()) {
            future.complete(true);
        } else {
            futureField.set(curatorInstance, CompletableFuture.completedFuture(true));
        }

        return new ZooKeeperKVStore();
    }

    @Override
    protected ControlPlaneConfiguration.KvStoreType kvStoreType() {
        return ControlPlaneConfiguration.KvStoreType.ZOOKEEPER;
    }

    @Override
    protected GenericContainer<?> container() {
        return ZK;
    }
}
