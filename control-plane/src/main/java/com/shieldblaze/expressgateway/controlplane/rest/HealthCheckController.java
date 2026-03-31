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
package com.shieldblaze.expressgateway.controlplane.rest;

import com.shieldblaze.expressgateway.controlplane.cluster.ConfigWriteForwarder;
import com.shieldblaze.expressgateway.controlplane.cluster.ControlPlaneCluster;
import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.types.HealthCheckSpec;
import com.shieldblaze.expressgateway.controlplane.distribution.ConfigDistributor;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.rest.dto.ApiResponse;
import com.shieldblaze.expressgateway.controlplane.rest.dto.HealthCheckDto;
import jakarta.annotation.Nullable;
import lombok.extern.log4j.Log4j2;

import static com.shieldblaze.expressgateway.common.utils.LogSanitizer.sanitize;

import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.PutMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import java.util.List;
import java.util.Optional;

/**
 * REST controller for health check configuration management.
 *
 * <p>Provides CRUD operations for health check configs. Each mutation is persisted
 * to the KV store and submitted to the {@link ConfigDistributor} for propagation
 * to connected data-plane nodes.</p>
 */
@RestController
@RequestMapping("/api/v1/controlplane/health-checks")
@Log4j2
public class HealthCheckController {

    private final KVStore kvStore;
    private final ConfigDistributor distributor;
    private final ControlPlaneCluster cluster;
    private final ConfigWriteForwarder forwarder;

    public HealthCheckController(KVStore kvStore, ConfigDistributor distributor,
                                 @Nullable ControlPlaneCluster cluster,
                                 @Nullable ConfigWriteForwarder forwarder) {
        this.kvStore = kvStore;
        this.distributor = distributor;
        this.cluster = cluster;
        this.forwarder = forwarder;
    }

    @PostMapping
    public ApiResponse<String> createHealthCheck(@RequestBody HealthCheckDto dto,
                                                 @RequestParam(defaultValue = "global") String scope) throws Exception {
        HealthCheckSpec spec = dto.toSpec();
        spec.validate();

        ConfigResource resource = ConfigResourceHelper.createResource(
                ConfigKind.HEALTH_CHECK, dto.name(), spec, null, scope);
        ConfigResourceHelper.persistAndDistributeWithLeaderCheck(resource, kvStore, distributor, cluster, forwarder);

        log.info("Created health check: {}", dto.name());
        return ApiResponse.ok("Health check created", dto.name());
    }

    @GetMapping
    public ApiResponse<List<HealthCheckDto>> listHealthChecks(
            @RequestParam(defaultValue = "global") String scope) throws Exception {
        List<ConfigResource> resources = ConfigResourceHelper.listResources(ConfigKind.HEALTH_CHECK, scope, kvStore);
        List<HealthCheckDto> dtos = resources.stream()
                .map(r -> HealthCheckDto.from((HealthCheckSpec) r.spec()))
                .toList();
        return ApiResponse.ok(dtos);
    }

    @GetMapping("/{healthCheckId}")
    public ApiResponse<HealthCheckDto> getHealthCheck(@PathVariable String healthCheckId,
                                                      @RequestParam(defaultValue = "global") String scope) throws Exception {
        ConfigResourceId id = new ConfigResourceId(ConfigKind.HEALTH_CHECK.name(), scope, healthCheckId);
        Optional<ConfigResource> resource = ConfigResourceHelper.getResource(id, kvStore);
        if (resource.isEmpty()) {
            return ApiResponse.error("Health check not found: " + healthCheckId);
        }
        return ApiResponse.ok(HealthCheckDto.from((HealthCheckSpec) resource.get().spec()));
    }

    @PutMapping("/{healthCheckId}")
    public ApiResponse<String> updateHealthCheck(@PathVariable String healthCheckId, @RequestBody HealthCheckDto dto,
                                                 @RequestParam(defaultValue = "global") String scope) throws Exception {
        ConfigResourceId id = new ConfigResourceId(ConfigKind.HEALTH_CHECK.name(), scope, healthCheckId);
        Optional<ConfigResource> existing = ConfigResourceHelper.getResource(id, kvStore);
        if (existing.isEmpty()) {
            return ApiResponse.error("Health check not found: " + healthCheckId);
        }

        HealthCheckSpec spec = dto.toSpec();
        spec.validate();

        ConfigResource updated = ConfigResourceHelper.updateResource(existing.get(), spec, null);
        ConfigResourceHelper.persistAndDistributeWithLeaderCheck(updated, kvStore, distributor, cluster, forwarder);

        log.info("Updated health check: {}", sanitize(healthCheckId));
        return ApiResponse.ok("Health check updated", healthCheckId);
    }

    @DeleteMapping("/{healthCheckId}")
    public ApiResponse<String> deleteHealthCheck(@PathVariable String healthCheckId,
                                                 @RequestParam(defaultValue = "global") String scope) throws Exception {
        ConfigResourceId id = new ConfigResourceId(ConfigKind.HEALTH_CHECK.name(), scope, healthCheckId);
        ConfigResourceHelper.deleteAndDistributeWithLeaderCheck(id, kvStore, distributor, cluster, forwarder);

        log.info("Deleted health check: {}", sanitize(healthCheckId));
        return ApiResponse.ok("Health check deleted", healthCheckId);
    }
}
