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
package com.shieldblaze.expressgateway.coordination.zookeeper;

import com.shieldblaze.expressgateway.coordination.CoordinationException;
import com.shieldblaze.expressgateway.coordination.CoordinationProvider;
import com.shieldblaze.expressgateway.coordination.CoordinationProviderFactory;
import lombok.extern.log4j.Log4j2;
import org.apache.curator.RetryPolicy;
import org.apache.curator.framework.CuratorFramework;
import org.apache.curator.framework.CuratorFrameworkFactory;
import org.apache.curator.retry.RetryNTimes;

import java.util.Map;
import java.util.Objects;
import java.util.concurrent.TimeUnit;

/**
 * Factory for creating {@link ZooKeeperCoordinationProvider} instances from configuration maps.
 *
 * <h2>Supported configuration keys</h2>
 * <table>
 *   <tr><th>Key</th><th>Required</th><th>Default</th><th>Description</th></tr>
 *   <tr><td>{@code connectionString}</td><td>Yes</td><td>-</td><td>ZK connection string (e.g. "host1:2181,host2:2181")</td></tr>
 *   <tr><td>{@code sessionTimeoutMs}</td><td>No</td><td>60000</td><td>ZK session timeout in milliseconds</td></tr>
 *   <tr><td>{@code connectionTimeoutMs}</td><td>No</td><td>15000</td><td>Connection timeout in milliseconds</td></tr>
 *   <tr><td>{@code retryCount}</td><td>No</td><td>3</td><td>Number of retries for transient failures</td></tr>
 *   <tr><td>{@code retrySleepMs}</td><td>No</td><td>1000</td><td>Sleep between retries in milliseconds</td></tr>
 * </table>
 */
@Log4j2
public final class ZooKeeperProviderFactory implements CoordinationProviderFactory {

    /** Default ZK session timeout: 60 seconds. */
    private static final int DEFAULT_SESSION_TIMEOUT_MS = 60_000;

    /** Default connection timeout: 15 seconds. */
    private static final int DEFAULT_CONNECTION_TIMEOUT_MS = 15_000;

    /** Default retry count for transient failures. */
    private static final int DEFAULT_RETRY_COUNT = 3;

    /** Default sleep between retries: 1 second. */
    private static final int DEFAULT_RETRY_SLEEP_MS = 1_000;

    /** Maximum time to wait for initial connection: 30 seconds. */
    private static final int CONNECTION_WAIT_TIMEOUT_SECONDS = 30;

    @Override
    public String type() {
        return "zookeeper";
    }

    @Override
    public CoordinationProvider create(Map<String, String> config) throws CoordinationException {
        Objects.requireNonNull(config, "config");

        String connectionString = config.get("connectionString");
        if (connectionString == null || connectionString.isBlank()) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Required configuration key 'connectionString' is missing or blank");
        }

        int sessionTimeoutMs = getInt(config, "sessionTimeoutMs", DEFAULT_SESSION_TIMEOUT_MS);
        int connectionTimeoutMs = getInt(config, "connectionTimeoutMs", DEFAULT_CONNECTION_TIMEOUT_MS);
        int retryCount = getInt(config, "retryCount", DEFAULT_RETRY_COUNT);
        int retrySleepMs = getInt(config, "retrySleepMs", DEFAULT_RETRY_SLEEP_MS);

        RetryPolicy retryPolicy = new RetryNTimes(retryCount, retrySleepMs);

        CuratorFramework client = CuratorFrameworkFactory.builder()
                .connectString(connectionString)
                .sessionTimeoutMs(sessionTimeoutMs)
                .connectionTimeoutMs(connectionTimeoutMs)
                .retryPolicy(retryPolicy)
                .build();

        client.start();

        try {
            if (!client.blockUntilConnected(CONNECTION_WAIT_TIMEOUT_SECONDS, TimeUnit.SECONDS)) {
                client.close();
                throw new CoordinationException(CoordinationException.Code.CONNECTION_LOST,
                        "Failed to connect to ZooKeeper at " + connectionString
                                + " within " + CONNECTION_WAIT_TIMEOUT_SECONDS + " seconds");
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            client.close();
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Interrupted while connecting to ZooKeeper at " + connectionString, e);
        }

        log.info("Connected to ZooKeeper at {} (session={}ms, connection={}ms, retries={}x{}ms)",
                connectionString, sessionTimeoutMs, connectionTimeoutMs, retryCount, retrySleepMs);

        // Transfer ownership: the provider will close this client on provider.close()
        return new ZooKeeperCoordinationProvider(client, true);
    }

    /**
     * Extracts an integer from the config map with a default fallback.
     */
    private static int getInt(Map<String, String> config, String key, int defaultValue) {
        String value = config.get(key);
        if (value == null || value.isBlank()) {
            return defaultValue;
        }
        try {
            return Integer.parseInt(value.trim());
        } catch (NumberFormatException e) {
            log.warn("Invalid integer value for config key '{}': '{}', using default {}",
                    key, value, defaultValue);
            return defaultValue;
        }
    }
}
