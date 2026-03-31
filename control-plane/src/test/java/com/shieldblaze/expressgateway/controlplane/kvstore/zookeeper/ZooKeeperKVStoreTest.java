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
package com.shieldblaze.expressgateway.controlplane.kvstore.zookeeper;

import com.shieldblaze.expressgateway.common.zookeeper.Curator;
import com.shieldblaze.expressgateway.controlplane.kvstore.AbstractKVStoreTest;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import org.apache.curator.framework.CuratorFramework;
import org.apache.curator.framework.CuratorFrameworkFactory;
import org.apache.curator.retry.RetryNTimes;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.testcontainers.containers.GenericContainer;
import org.testcontainers.containers.wait.strategy.Wait;
import org.testcontainers.junit.jupiter.Container;
import org.testcontainers.junit.jupiter.Testcontainers;

import java.io.IOException;
import java.lang.reflect.Field;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

/**
 * Testcontainers-based integration test for {@link ZooKeeperKVStore}.
 *
 * <p>Runs a real ZooKeeper 3.9 instance in a Docker container and executes the full
 * {@link AbstractKVStoreTest} suite against it. The container is started once per
 * class (shared across all test methods).</p>
 *
 * <p>{@link ZooKeeperKVStore} uses the {@link Curator} singleton to obtain a
 * {@link CuratorFramework} instance. Since the singleton's {@code init()} method
 * depends on the {@code ExpressGateway} configuration (which is not available in
 * tests), this class uses reflection to inject a test-configured CuratorFramework
 * directly into the singleton. This is the safest approach for testing without
 * modifying the production Curator class.</p>
 */
@Testcontainers
class ZooKeeperKVStoreTest extends AbstractKVStoreTest {

    @Container
    static final GenericContainer<?> ZK = new GenericContainer<>("zookeeper:3.9")
            .withExposedPorts(2181)
            .waitingFor(Wait.forListeningPort());

    private static ZooKeeperKVStore store;
    private static CuratorFramework curatorFramework;

    @BeforeAll
    static void setUp() throws Exception {
        String connectString = ZK.getHost() + ":" + ZK.getMappedPort(2181);

        // Build a CuratorFramework connected to the test container
        curatorFramework = CuratorFrameworkFactory.builder()
                .connectString(connectString)
                .retryPolicy(new RetryNTimes(3, 1000))
                .build();
        curatorFramework.start();
        curatorFramework.blockUntilConnected(30, TimeUnit.SECONDS);

        // Inject the CuratorFramework into the Curator singleton via reflection.
        // The Curator class has a private static final INSTANCE field holding the singleton,
        // and the singleton has a private 'curatorFramework' field and an 'initializationFuture'.
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
            // Replace with a completed future if the existing one is already done
            futureField.set(curatorInstance, CompletableFuture.completedFuture(true));
        }

        store = new ZooKeeperKVStore();
    }

    @AfterAll
    static void tearDown() throws IOException {
        if (store != null) {
            store.close();
        }
        if (curatorFramework != null) {
            curatorFramework.close();
        }
    }

    @Override
    protected KVStore kvStore() {
        return store;
    }
}
