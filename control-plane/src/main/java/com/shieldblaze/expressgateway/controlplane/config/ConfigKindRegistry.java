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

import lombok.extern.log4j.Log4j2;

import java.util.Collection;
import java.util.Collections;
import java.util.Objects;
import java.util.Optional;
import java.util.ServiceLoader;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Central registry for {@link ConfigKindRegistration} entries, populated via
 * {@link java.util.ServiceLoader} and programmatic registration.
 *
 * <p>On class load, the registry discovers all {@link ConfigKindProvider} implementations
 * on the classpath and registers them. Additional registrations can be added programmatically
 * via {@link #register(ConfigKindRegistration)}.</p>
 *
 * <p>This registry is thread-safe and uses {@link ConcurrentHashMap} for lock-free reads
 * on the hot path.</p>
 */
@Log4j2
public final class ConfigKindRegistry {

    private static final ConcurrentHashMap<String, ConfigKindRegistration> REGISTRY = new ConcurrentHashMap<>();

    static {
        loadProviders();
    }

    private ConfigKindRegistry() {
        // Static utility class
    }

    private static void loadProviders() {
        ServiceLoader.load(ConfigKindProvider.class).forEach(provider -> {
            ConfigKindRegistration reg = provider.registration();
            ConfigKindRegistration existing = REGISTRY.putIfAbsent(reg.kind().name(), reg);
            if (existing != null) {
                log.warn("Duplicate ConfigKindProvider for kind '{}': ignoring {} in favor of {}",
                        reg.kind().name(), reg.specClass().getName(), existing.specClass().getName());
            } else {
                log.info("Registered config kind '{}' -> {}", reg.kind().name(), reg.specClass().getName());
            }
        });
    }

    /**
     * Retrieve the registration for a given kind name.
     *
     * @param kindName The kind name to look up
     * @return The registration, or {@link Optional#empty()} if not registered
     */
    public static Optional<ConfigKindRegistration> get(String kindName) {
        Objects.requireNonNull(kindName, "kindName");
        return Optional.ofNullable(REGISTRY.get(kindName));
    }

    /**
     * Programmatically register a config kind. Replaces any existing registration
     * for the same kind name.
     *
     * @param registration The registration to add
     */
    public static void register(ConfigKindRegistration registration) {
        Objects.requireNonNull(registration, "registration");
        ConfigKindRegistration previous = REGISTRY.put(registration.kind().name(), registration);
        if (previous != null) {
            log.info("Replaced config kind '{}': {} -> {}",
                    registration.kind().name(), previous.specClass().getName(), registration.specClass().getName());
        } else {
            log.info("Registered config kind '{}' -> {}",
                    registration.kind().name(), registration.specClass().getName());
        }
    }

    /**
     * Returns all registered config kind registrations.
     *
     * @return An unmodifiable view of all registrations
     */
    public static Collection<ConfigKindRegistration> all() {
        return Collections.unmodifiableCollection(REGISTRY.values());
    }
}
