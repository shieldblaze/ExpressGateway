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
package com.shieldblaze.expressgateway.common;

public final class SystemPropertiesKeys {

    public static final String SYSTEM_ID = "system.id";
    public static final String REST_API_IP_ADDRESS = "rest-api.ipAddress";
    public static final String REST_API_PORT = "rest-api.port";
    public static final String MONGODB_CONNECTION_STRING = "mongodb.connection-string";
    public static final String CLUSTER_ID = "mongodb.cluster-id";
    public static final String CRYPTO_PASSWORD = "crypto.password";

    public static final String ZOOKEEPER_ADDRESS = "zookeeper-address";

    private SystemPropertiesKeys() {
        // Prevent outside initialization
    }
}
