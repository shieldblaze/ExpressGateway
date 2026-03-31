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

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;

/**
 * Represents an atomic multi-resource configuration commit.
 *
 * <p>A transaction groups one or more {@link ConfigMutation} operations that must
 * be applied together. All mutations within a transaction succeed or fail as a unit.
 * Transactions carry an author and optional description for audit purposes.</p>
 *
 * <p>Use {@link #builder(String)} to construct transactions:</p>
 * <pre>{@code
 * ConfigTransaction tx = ConfigTransaction.builder("admin@example.com")
 *     .description("Add production cluster")
 *     .upsert(clusterResource)
 *     .upsert(listenerResource)
 *     .build();
 * }</pre>
 */
public final class ConfigTransaction {

    private final List<ConfigMutation> mutations;
    private final String author;
    private final String description;

    private ConfigTransaction(List<ConfigMutation> mutations, String author, String description) {
        this.mutations = Collections.unmodifiableList(new ArrayList<>(mutations));
        this.author = author;
        this.description = description;
    }

    /**
     * Create a new transaction builder.
     *
     * @param author The principal authoring this transaction (e.g. username or service identity)
     * @return A new builder instance
     */
    public static Builder builder(String author) {
        return new Builder(author);
    }

    /**
     * Returns the ordered list of mutations in this transaction.
     */
    public List<ConfigMutation> mutations() {
        return Collections.unmodifiableList(mutations);
    }

    /**
     * Returns the author of this transaction.
     */
    public String author() {
        return author;
    }

    /**
     * Returns the optional description of this transaction.
     * May be {@code null} if no description was provided.
     */
    public String description() {
        return description;
    }

    /**
     * Builder for constructing {@link ConfigTransaction} instances.
     */
    public static final class Builder {

        private final String author;
        private final List<ConfigMutation> mutations = new ArrayList<>();
        private String description;

        private Builder(String author) {
            Objects.requireNonNull(author, "author");
            if (author.isBlank()) {
                throw new IllegalArgumentException("author must not be blank");
            }
            this.author = author;
        }

        /**
         * Add an upsert mutation for the given resource.
         *
         * @param resource The resource to create or update
         * @return This builder
         */
        public Builder upsert(ConfigResource resource) {
            Objects.requireNonNull(resource, "resource");
            mutations.add(new ConfigMutation.Upsert(resource));
            return this;
        }

        /**
         * Add a delete mutation for the given resource ID.
         *
         * @param id The ID of the resource to delete
         * @return This builder
         */
        public Builder delete(ConfigResourceId id) {
            Objects.requireNonNull(id, "id");
            mutations.add(new ConfigMutation.Delete(id));
            return this;
        }

        /**
         * Set an optional human-readable description for this transaction.
         *
         * @param desc The description
         * @return This builder
         */
        public Builder description(String desc) {
            this.description = desc;
            return this;
        }

        /**
         * Build the transaction.
         *
         * @return The immutable {@link ConfigTransaction}
         * @throws IllegalStateException if no mutations have been added
         */
        public ConfigTransaction build() {
            if (mutations.isEmpty()) {
                throw new IllegalStateException("Transaction must contain at least one mutation");
            }
            return new ConfigTransaction(mutations, author, description);
        }
    }
}
