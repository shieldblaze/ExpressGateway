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
package com.shieldblaze.expressgateway.config;

import com.fasterxml.jackson.databind.DeserializationFeature;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import com.fasterxml.jackson.dataformat.yaml.YAMLMapper;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import java.util.Objects;
import java.util.function.Function;

/**
 * Loads {@link GatewayConfig} with layered resolution.
 *
 * <p>Resolution order (each layer overrides the previous):</p>
 * <ol>
 *   <li>Builder defaults (coded in the {@code @Builder.Default} annotations)</li>
 *   <li>Config file (JSON or YAML, auto-detected by file extension)</li>
 *   <li>Environment variables (prefix {@code EG_})</li>
 *   <li>System properties (prefix {@code expressgateway.})</li>
 *   <li>Explicit overrides (if any are passed)</li>
 * </ol>
 *
 * <p>After loading, the config is validated. If validation fails,
 * {@link ConfigValidationException} is thrown with all violations.</p>
 *
 * <p>This class has no mutable state and all methods are static.</p>
 */
public final class ConfigLoader {

    private static final String DEFAULT_CONFIG_PATH = "/etc/expressgateway/gateway.yaml";
    private static final String CONFIG_ENV_VAR = "EG_CONFIG_FILE";

    private static final ObjectMapper JSON_MAPPER = createJsonMapper();
    private static final YAMLMapper YAML_MAPPER = createYamlMapper();

    private ConfigLoader() {
        // utility class
    }

    /**
     * Loads configuration from the specified file with environment variable and
     * system property overlays applied.
     *
     * @param configFile path to a JSON or YAML configuration file
     * @return the validated {@link GatewayConfig}
     * @throws ConfigValidationException if validation fails
     * @throws IOException               if the file cannot be read or parsed
     */
    public static GatewayConfig load(Path configFile) throws IOException {
        return load(configFile, Map.of());
    }

    /**
     * Loads configuration from the specified file with environment variable,
     * system property, and explicit overrides applied.
     *
     * @param configFile path to a JSON or YAML configuration file
     * @param overrides  explicit key-value overrides (highest precedence).
     *                   Keys can be JSON pointer paths (e.g. "/clusterId") or
     *                   dot-separated paths (e.g. "restApi.port").
     * @return the validated {@link GatewayConfig}
     * @throws ConfigValidationException if validation fails
     * @throws IOException               if the file cannot be read or parsed
     */
    public static GatewayConfig load(Path configFile, Map<String, String> overrides) throws IOException {
        return load(configFile, overrides, System::getenv, System::getProperty);
    }

    /**
     * Loads configuration from the environment. The config file path is resolved from
     * the {@code EG_CONFIG_FILE} environment variable, falling back to
     * {@code /etc/expressgateway/gateway.yaml}.
     *
     * @return the validated {@link GatewayConfig}
     * @throws ConfigValidationException if validation fails
     * @throws IOException               if the file cannot be read or parsed
     */
    public static GatewayConfig fromEnvironment() throws IOException {
        String configPath = System.getenv(CONFIG_ENV_VAR);
        if (configPath == null || configPath.isBlank()) {
            configPath = DEFAULT_CONFIG_PATH;
        }
        return load(Path.of(configPath));
    }

    /**
     * Internal load method that accepts injectable lookup functions for testing.
     */
    static GatewayConfig load(Path configFile, Map<String, String> overrides,
                              Function<String, String> envLookup,
                              Function<String, String> sysLookup) throws IOException {
        Objects.requireNonNull(configFile, "configFile must not be null");
        Objects.requireNonNull(overrides, "overrides must not be null");

        // Step 1: Parse the config file into a mutable Jackson tree
        ObjectMapper mapper = selectMapper(configFile);
        ObjectNode tree;
        try (InputStream in = Files.newInputStream(configFile)) {
            tree = mapper.readValue(in, ObjectNode.class);
        }

        // Step 2: Apply environment variable overlay
        // Step 3: Apply system property overlay
        ConfigOverlay.apply(tree, mapper, envLookup, sysLookup);

        // Step 4: Apply explicit overrides (highest precedence)
        if (!overrides.isEmpty()) {
            ConfigOverlay.applyOverrides(tree, mapper, overrides);
        }

        // Step 5: Bind tree to GatewayConfig
        GatewayConfig config = mapper.treeToValue(tree, GatewayConfig.class);

        // Step 6: Validate
        config.validate();

        return config;
    }

    /**
     * Selects the appropriate Jackson mapper based on file extension.
     * YAML: .yaml, .yml
     * JSON: everything else (including .json)
     */
    private static ObjectMapper selectMapper(Path configFile) {
        String fileName = configFile.getFileName().toString().toLowerCase();
        if (fileName.endsWith(".yaml") || fileName.endsWith(".yml")) {
            return YAML_MAPPER;
        }
        return JSON_MAPPER;
    }

    private static ObjectMapper createJsonMapper() {
        ObjectMapper mapper = new ObjectMapper();
        mapper.configure(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES, false);
        mapper.configure(DeserializationFeature.READ_ENUMS_USING_TO_STRING, true);
        return mapper;
    }

    private static YAMLMapper createYamlMapper() {
        YAMLMapper mapper = new YAMLMapper();
        mapper.configure(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES, false);
        mapper.configure(DeserializationFeature.READ_ENUMS_USING_TO_STRING, true);
        return mapper;
    }
}
