/**
 * Copyright 2020 Alibaba Group Holding Limited.
 *
 * <p>Licensed under the Apache License, Version 2.0 (the "License"); you may not use this file
 * except in compliance with the License. You may obtain a copy of the License at
 *
 * <p>http://www.apache.org/licenses/LICENSE-2.0
 *
 * <p>Unless required by applicable law or agreed to in writing, software distributed under the
 * License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either
 * express or implied. See the License for the specific language governing permissions and
 * limitations under the License.
 */
package com.alibaba.graphscope.groot.dataload.databuild;

import com.alibaba.graphscope.groot.common.config.DataLoadConfig;
import com.alibaba.graphscope.groot.common.schema.api.GraphEdge;
import com.alibaba.graphscope.groot.common.schema.api.GraphElement;
import com.alibaba.graphscope.groot.common.schema.api.GraphSchema;
import com.alibaba.graphscope.groot.common.schema.mapper.GraphSchemaMapper;
import com.alibaba.graphscope.groot.common.schema.wrapper.GraphDef;
import com.alibaba.graphscope.groot.common.util.UuidUtils;
import com.alibaba.graphscope.groot.sdk.GrootClient;
import com.alibaba.graphscope.proto.groot.DataLoadTargetPb;
import com.fasterxml.jackson.core.type.TypeReference;
import com.fasterxml.jackson.databind.ObjectMapper;

import org.apache.hadoop.conf.Configuration;
import org.apache.hadoop.fs.FSDataOutputStream;
import org.apache.hadoop.fs.FileSystem;
import org.apache.hadoop.fs.Path;
import org.apache.hadoop.io.BytesWritable;
import org.apache.hadoop.mapreduce.Job;
import org.apache.hadoop.mapreduce.lib.input.CombineTextInputFormat;
import org.apache.hadoop.mapreduce.lib.input.FileInputFormat;
import org.apache.hadoop.mapreduce.lib.output.FileOutputFormat;
import org.apache.hadoop.mapreduce.lib.output.LazyOutputFormat;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.*;

public class OfflineBuild {
    private static final Logger logger = LoggerFactory.getLogger(OfflineBuild.class);

    public static void main(String[] args)
            throws IOException, ClassNotFoundException, InterruptedException {
        String propertiesFile = args[0];
        Properties properties = new Properties();
        try (InputStream is = new FileInputStream(propertiesFile)) {
            properties.load(is);
        }
        String inputPath = properties.getProperty(DataLoadConfig.INPUT_PATH);
        String outputPath = properties.getProperty(DataLoadConfig.OUTPUT_PATH);
        String columnMappingConfigStr =
                properties.getProperty(DataLoadConfig.COLUMN_MAPPING_CONFIG);
        String graphEndpoint = properties.getProperty(DataLoadConfig.GRAPH_ENDPOINT);

        String uniquePath =
                properties.getProperty(DataLoadConfig.UNIQUE_PATH, UuidUtils.getBase64UUIDString());

        String username = properties.getProperty(DataLoadConfig.USER_NAME, "");
        String password = properties.getProperty(DataLoadConfig.PASS_WORD, "");

        GrootClient client =
                GrootClient.newBuilder()
                        .setHosts(graphEndpoint)
                        .setUsername(username)
                        .setPassword(password)
                        .build();
        ObjectMapper objectMapper = new ObjectMapper();
        Map<String, FileColumnMapping> columnMappingConfig =
                objectMapper.readValue(
                        columnMappingConfigStr,
                        new TypeReference<Map<String, FileColumnMapping>>() {});

        List<DataLoadTargetPb> targets = new ArrayList<>();
        for (FileColumnMapping fileColumnMapping : columnMappingConfig.values()) {
            DataLoadTargetPb.Builder builder = DataLoadTargetPb.newBuilder();
            builder.setLabel(fileColumnMapping.getLabel());
            if (fileColumnMapping.getSrcLabel() != null) {
                builder.setSrcLabel(fileColumnMapping.getSrcLabel());
            }
            if (fileColumnMapping.getDstLabel() != null) {
                builder.setDstLabel(fileColumnMapping.getDstLabel());
            }
            targets.add(builder.build());
        }
        GraphSchema schema = GraphDef.parseProto(client.prepareDataLoad(targets));
        String schemaJson = GraphSchemaMapper.parseFromSchema(schema).toJsonString();
        int partitionNum = client.getPartitionNum();

        Map<String, ColumnMappingInfo> columnMappingInfos = new HashMap<>();
        columnMappingConfig.forEach(
                (fileName, fileColumnMapping) -> {
                    columnMappingInfos.put(fileName, fileColumnMapping.toColumnMappingInfo(schema));
                });
        String ldbcCustomize = properties.getProperty(DataLoadConfig.LDBC_CUSTOMIZE, "true");
        long splitSize =
                Long.parseLong(properties.getProperty(DataLoadConfig.SPLIT_SIZE, "256"))
                        * 1024
                        * 1024;
        boolean loadAfterBuild =
                properties
                        .getProperty(DataLoadConfig.LOAD_AFTER_BUILD, "false")
                        .equalsIgnoreCase("true");
        boolean skipHeader =
                properties.getProperty(DataLoadConfig.SKIP_HEADER, "true").equalsIgnoreCase("true");
        String separator = properties.getProperty(DataLoadConfig.SEPARATOR, "\\|");

        Configuration conf = new Configuration();
        conf.setBoolean("mapreduce.map.speculative", false);
        conf.setBoolean("mapreduce.reduce.speculative", false);
        conf.setLong(CombineTextInputFormat.SPLIT_MINSIZE_PERNODE, splitSize);
        conf.setLong(CombineTextInputFormat.SPLIT_MINSIZE_PERRACK, splitSize);
        conf.setStrings(DataLoadConfig.SCHEMA_JSON, schemaJson);
        String mappings = objectMapper.writeValueAsString(columnMappingInfos);
        conf.setStrings(DataLoadConfig.COLUMN_MAPPINGS, mappings);
        conf.setBoolean(DataLoadConfig.LDBC_CUSTOMIZE, ldbcCustomize.equalsIgnoreCase("true"));
        conf.set(DataLoadConfig.SEPARATOR, separator);
        conf.setBoolean(DataLoadConfig.SKIP_HEADER, skipHeader);
        Job job = Job.getInstance(conf, "build graph data");
        job.setJarByClass(OfflineBuild.class);
        job.setMapperClass(DataBuildMapper.class);
        job.setPartitionerClass(DataBuildPartitioner.class);
        job.setReducerClass(DataBuildReducer.class);
        job.setNumReduceTasks(partitionNum);
        job.setOutputKeyClass(BytesWritable.class);
        job.setOutputValueClass(BytesWritable.class);
        job.setInputFormatClass(CombineTextInputFormat.class);
        CombineTextInputFormat.setMaxInputSplitSize(job, splitSize);
        LazyOutputFormat.setOutputFormatClass(job, SstOutputFormat.class);
        FileInputFormat.addInputPath(job, new Path(inputPath));
        FileInputFormat.setInputDirRecursive(job, true);

        Path outputDir = new Path(outputPath, uniquePath);
        FileOutputFormat.setOutputPath(job, outputDir);
        if (!job.waitForCompletion(true)) {
            System.exit(1);
        }

        Map<String, String> outputMeta = new HashMap<>();
        outputMeta.put(DataLoadConfig.GRAPH_ENDPOINT, graphEndpoint);
        outputMeta.put(DataLoadConfig.SCHEMA_JSON, schemaJson);
        outputMeta.put(DataLoadConfig.COLUMN_MAPPINGS, mappings);
        outputMeta.put(DataLoadConfig.UNIQUE_PATH, uniquePath);

        FileSystem fs = outputDir.getFileSystem(job.getConfiguration());
        FSDataOutputStream os = fs.create(new Path(outputDir, "META"));
        os.writeUTF(objectMapper.writeValueAsString(outputMeta));
        os.flush();
        os.close();

        if (loadAfterBuild) {
            String dataPath = fs.makeQualified(outputDir).toString();

            logger.info("start ingesting data");
            client.ingestData(dataPath);

            logger.info("commit bulk load");
            Map<Long, DataLoadTargetPb> tableToTarget = new HashMap<>();
            for (ColumnMappingInfo columnMappingInfo : columnMappingInfos.values()) {
                DataLoadTargetPb.Builder builder = DataLoadTargetPb.newBuilder();
                GraphElement graphElement = schema.getElement(columnMappingInfo.getLabelId());
                builder.setLabel(graphElement.getLabel());
                if (graphElement instanceof GraphEdge) {
                    builder.setSrcLabel(
                            schema.getElement(columnMappingInfo.getSrcLabelId()).getLabel());
                    builder.setDstLabel(
                            schema.getElement(columnMappingInfo.getDstLabelId()).getLabel());
                }
                tableToTarget.put(columnMappingInfo.getTableId(), builder.build());
            }
            String ingest_behind = properties.getProperty(DataLoadConfig.INGEST_BEHIND, "false");
            Map<String, String> options = new HashMap<>();
            options.put(DataLoadConfig.INGEST_BEHIND, ingest_behind);
            client.commitDataLoad(tableToTarget, uniquePath, options);
        }
    }
}
