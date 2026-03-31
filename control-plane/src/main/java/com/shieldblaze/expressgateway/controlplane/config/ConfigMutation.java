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
package com.shieldblaze.expressgateway.controlplane.config;

import com.fasterxml.jackson.annotation.JsonSubTypes;
import com.fasterxml.jackson.annotation.JsonTypeInfo;

import java.util.Objects;

/**
 * Represents a single configuration change operation.
 *
 * <p>Mutations are the atomic units of change within a {@link ConfigTransaction}.
 * They are sealed to the two possible operations: creating/updating a resource
 * ({@link Upsert}) or removing one ({@link Delete}).</p>
 */
@JsonTypeInfo(use = JsonTypeInfo.Id.NAME, property = "@type")
@JsonSubTypes({
        @JsonSubTypes.Type(value = ConfigMutation.Upsert.class, name = "upsert"),
        @JsonSubTypes.Type(value = ConfigMutation.Delete.class, name = "delete")
})
public sealed interface ConfigMutation permits ConfigMutation.Upsert, ConfigMutation.Delete {

    /**
     * An upsert mutation: creates the resource if it does not exist, or updates it if it does.
     *
     * @param resource The full resource to write
     */
    record Upsert(ConfigResource resource) implements ConfigMutation {
        public Upsert {
            Objects.requireNonNull(resource, "resource");
        }
    }

    /**
     * A delete mutation: removes the resource identified by the given ID.
     *
     * @param resourceId The ID of the resource to delete
     */
    record Delete(ConfigResourceId resourceId) implements ConfigMutation {
        public Delete {
            Objects.requireNonNull(resourceId, "resourceId");
        }
    }
}
