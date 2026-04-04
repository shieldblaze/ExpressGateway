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
import com.shieldblaze.expressgateway.controlplane.config.types.RoutingRuleSpec;
import com.shieldblaze.expressgateway.controlplane.distribution.ConfigDistributor;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.rest.dto.ApiResponse;
import com.shieldblaze.expressgateway.controlplane.rest.dto.RouteDto;
import jakarta.annotation.Nullable;
import lombok.extern.log4j.Log4j2;

import static com.shieldblaze.expressgateway.common.utils.LogSanitizer.sanitize;

import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
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
 * REST controller for L7 routing rule configuration management.
 *
 * <p>Provides CRUD operations for routing rule configs. Each mutation is persisted
 * to the KV store and submitted to the {@link ConfigDistributor} for propagation
 * to connected data-plane nodes.</p>
 */
@RestController
@RequestMapping("/api/v1/controlplane/routes")
@Log4j2
public class RouteController {

    private final KVStore kvStore;
    private final ConfigDistributor distributor;
    private final ControlPlaneCluster cluster;
    private final ConfigWriteForwarder forwarder;

    public RouteController(KVStore kvStore, ConfigDistributor distributor,
                           @Nullable ControlPlaneCluster cluster,
                           @Nullable ConfigWriteForwarder forwarder) {
        this.kvStore = kvStore;
        this.distributor = distributor;
        this.cluster = cluster;
        this.forwarder = forwarder;
    }

    @PostMapping
    public ApiResponse<String> createRoute(@RequestBody RouteDto dto,
                                           @RequestParam(defaultValue = "global") String scope) throws Exception {
        RoutingRuleSpec spec = dto.toSpec();
        spec.validate();

        ConfigResource resource = ConfigResourceHelper.createResource(
                ConfigKind.ROUTING_RULE, dto.name(), spec, null, scope);
        ConfigResourceHelper.persistAndDistributeWithLeaderCheck(resource, kvStore, distributor, cluster, forwarder);

        log.info("Created route: {}", dto.name());
        return ApiResponse.ok("Route created", dto.name());
    }

    @GetMapping
    public ApiResponse<List<RouteDto>> listRoutes(
            @RequestParam(defaultValue = "global") String scope) throws Exception {
        List<ConfigResource> resources = ConfigResourceHelper.listResources(ConfigKind.ROUTING_RULE, scope, kvStore);
        List<RouteDto> dtos = resources.stream()
                .map(r -> RouteDto.from((RoutingRuleSpec) r.spec()))
                .toList();
        return ApiResponse.ok(dtos);
    }

    @GetMapping("/{routeId}")
    public ResponseEntity<ApiResponse<RouteDto>> getRoute(@PathVariable String routeId,
                                          @RequestParam(defaultValue = "global") String scope) throws Exception {
        ConfigResourceId id = new ConfigResourceId(ConfigKind.ROUTING_RULE.name(), scope, routeId);
        Optional<ConfigResource> resource = ConfigResourceHelper.getResource(id, kvStore);
        if (resource.isEmpty()) {
            return ResponseEntity.status(HttpStatus.NOT_FOUND)
                    .body(ApiResponse.error("Route not found: " + routeId));
        }
        return ResponseEntity.ok(ApiResponse.ok(RouteDto.from((RoutingRuleSpec) resource.get().spec())));
    }

    @PutMapping("/{routeId}")
    public ResponseEntity<ApiResponse<String>> updateRoute(@PathVariable String routeId, @RequestBody RouteDto dto,
                                           @RequestParam(defaultValue = "global") String scope) throws Exception {
        ConfigResourceId id = new ConfigResourceId(ConfigKind.ROUTING_RULE.name(), scope, routeId);
        Optional<ConfigResource> existing = ConfigResourceHelper.getResource(id, kvStore);
        if (existing.isEmpty()) {
            return ResponseEntity.status(HttpStatus.NOT_FOUND)
                    .body(ApiResponse.error("Route not found: " + routeId));
        }

        RoutingRuleSpec spec = dto.toSpec();
        spec.validate();

        ConfigResource updated = ConfigResourceHelper.updateResource(existing.get(), spec, null);
        ConfigResourceHelper.persistAndDistributeWithLeaderCheck(updated, kvStore, distributor, cluster, forwarder);

        log.info("Updated route: {}", sanitize(routeId));
        return ResponseEntity.ok(ApiResponse.ok("Route updated", routeId));
    }

    @DeleteMapping("/{routeId}")
    public ApiResponse<String> deleteRoute(@PathVariable String routeId,
                                           @RequestParam(defaultValue = "global") String scope) throws Exception {
        ConfigResourceId id = new ConfigResourceId(ConfigKind.ROUTING_RULE.name(), scope, routeId);
        ConfigResourceHelper.deleteAndDistributeWithLeaderCheck(id, kvStore, distributor, cluster, forwarder);

        log.info("Deleted route: {}", sanitize(routeId));
        return ApiResponse.ok("Route deleted", routeId);
    }
}
