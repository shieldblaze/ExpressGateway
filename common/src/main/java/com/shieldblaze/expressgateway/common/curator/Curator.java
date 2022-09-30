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
package com.shieldblaze.expressgateway.common.curator;

import com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil;
import org.apache.curator.RetryPolicy;
import org.apache.curator.framework.CuratorFramework;
import org.apache.curator.framework.CuratorFrameworkFactory;
import org.apache.curator.retry.RetryNTimes;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.apache.zookeeper.ClientCnxnSocketNetty;

import java.io.Closeable;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;

import static org.apache.zookeeper.client.ZKClientConfig.ZOOKEEPER_CLIENT_CNXN_SOCKET;

public final class Curator implements Closeable {

    private static final Logger logger = LogManager.getLogger(Curator.class);
    private static final CompletableFuture<Boolean> CONNECTION_FUTURE = new CompletableFuture<>();
    private static final Curator INSTANCE = new Curator();

    private final CuratorFramework curatorFramework;

    private Curator() {
        // Use Netty
        System.setProperty(ZOOKEEPER_CLIENT_CNXN_SOCKET, ClientCnxnSocketNetty.class.getCanonicalName());

        int sleepMsBetweenRetries = 100;
        int maxRetries = 3;
        RetryPolicy retryPolicy = new RetryNTimes(maxRetries, sleepMsBetweenRetries);

        CuratorFrameworkFactory.Builder builder = CuratorFrameworkFactory.builder()
                .connectString(Objects.requireNonNull(SystemPropertyUtil.getPropertyOrEnv("ZOOKEEPER_ADDRESS"), "ZooKeeper address is required"))
                .retryPolicy(retryPolicy);

        curatorFramework = builder.build();
        curatorFramework.start();

        CONNECTION_FUTURE.completeAsync(() -> {
            for (int i = 0; i < 30; i++) {

                // When isConnected is true then connection has been established successfully
                if (curatorFramework.getZookeeperClient().isConnected()) {
                    logger.info("Started Apache Zookeeper Curator. Connected: {}", true);
                    return true;
                }

                try {
                    Thread.sleep(1000);
                } catch (InterruptedException e) {
                    logger.error("ConnectionFuture sleep thread interrupted", e);
                    throw new RuntimeException(e);
                }
            }

            logger.fatal("Failed to start Apache Zookeeper Curator");
            return false;
        });
    }

    /**
     * Returns {@link CuratorFramework} instance
     */
    public static CuratorFramework getInstance() {
        return INSTANCE.curatorFramework;
    }

    /**
     * This {@link CompletableFuture} returns {@link Boolean#TRUE} once
     * MongoDB connection has been successfully else returns {@link Boolean#FALSE}
     */
    public static CompletableFuture<Boolean> connectionFuture() {
        return CONNECTION_FUTURE;
    }

    /**
     * Calls {@link #close()}
     */
    public static void shutdown() {
        INSTANCE.close();
    }

    @Override
    public void close() {
        curatorFramework.close();
    }
}
