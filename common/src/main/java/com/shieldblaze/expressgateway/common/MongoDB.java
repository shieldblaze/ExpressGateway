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

import com.mongodb.client.MongoClient;
import com.mongodb.client.MongoClients;
import com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil;
import dev.morphia.Datastore;
import dev.morphia.Morphia;
import dev.morphia.mapping.MapperOptions;

import java.io.Closeable;

/**
 * This class holds {@link Datastore} for accessing
 * MongoDB database.
 */
public final class MongoDB implements Closeable {

    private static final boolean enabled;
    private static MongoClient mongoClient;

    /**
     * Morphia {@link Datastore} Instance
     */
    private static Datastore DATASTORE;

    static {
        enabled = SystemPropertyUtil.getPropertyOrEnvBoolean("mongodb", "false");
        if (enabled) {
            mongoClient = MongoClients.create(SystemPropertyUtil.getPropertyOrEnv("MONGO_CONNECTION_STRING"));
            DATASTORE = Morphia.createDatastore(mongoClient, SystemPropertyUtil.getPropertyOrEnv("MONGO_DATABASE"), MapperOptions.builder()
                    .storeNulls(true)
                    .storeEmpties(true)
                    .ignoreFinals(false)
                    .cacheClassLookups(true)
                    .build());
        }
    }

    public static Datastore getInstance() {
        if (!enabled()) {
            throw new UnsupportedOperationException("MongoDB support is disabled");
        }
        return DATASTORE;
    }

    /**
     * Returns {@code true} when MongoDB is enabled
     * else {@code false}
     */
    public static boolean enabled() {
        return enabled;
    }

    @Override
    public void close() {
        mongoClient.close();
    }
}
