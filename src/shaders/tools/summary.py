#!/usr/bin/env python3
"""Summary of nxshaders prebuild folders: per game, shaders found / translated / failed / spirv-val clean and the time
taken (from <id>.log beside each folder), the reasons for failures, and the SPIR-V capabilities over all outputs (the
device features the graphics module must enable).

  py src/shaders/tools/summary.py build/shaders mkw mk8d dkb oot za arceus violet
"""
import collections
import json
import os
import re
import sys

CAPS = {1: 'Shader', 2: 'Geometry', 3: 'Tessellation', 9: 'Float16', 10: 'Float64', 11: 'Int64', 12: 'Int64Atomics', 22: 'Int16',
        25: 'ImageGatherExtended', 27: 'StorageImageMultisample', 32: 'ClipDistance', 33: 'CullDistance', 34: 'ImageCubeArray',
        35: 'SampleRateShading', 43: 'Sampled1D', 44: 'Image1D', 45: 'SampledCubeArray', 46: 'SampledBuffer', 47: 'ImageBuffer',
        48: 'ImageMSArray', 49: 'StorageImageExtendedFormats', 50: 'ImageQuery', 51: 'DerivativeControl', 52: 'InterpolationFunction',
        53: 'TransformFeedback', 54: 'GeometryStreams', 55: 'StorageImageReadWithoutFormat', 56: 'StorageImageWriteWithoutFormat',
        57: 'MultiViewport', 61: 'GroupNonUniform', 62: 'GroupNonUniformVote', 63: 'GroupNonUniformArithmetic', 64: 'GroupNonUniformBallot',
        65: 'GroupNonUniformShuffle', 66: 'GroupNonUniformShuffleRelative', 67: 'GroupNonUniformClustered', 68: 'GroupNonUniformQuad',
        69: 'ShaderLayer', 70: 'ShaderViewportIndex', 4423: 'SubgroupBallotKHR', 4427: 'DrawParameters', 4431: 'SubgroupVoteKHR',
        4433: 'StorageBuffer16BitAccess', 4437: 'DeviceGroup', 4439: 'MultiView', 4447: 'SampleMaskPostDepthCoverage',
        4448: 'StorageBuffer8BitAccess', 5013: 'StencilExportEXT', 5251: 'GeometryShaderPassthroughNV', 5254: 'ShaderViewportIndexLayerEXT',
        5255: 'ShaderViewportMaskNV', 5301: 'ShaderNonUniform', 5306: 'UniformBufferArrayNonUniformIndexing',
        5307: 'SampledImageArrayNonUniformIndexing', 5308: 'StorageBufferArrayNonUniformIndexing', 5309: 'StorageImageArrayNonUniformIndexing',
        5345: 'VulkanMemoryModel', 5347: 'PhysicalStorageBufferAddresses', 5378: 'FragmentShaderPixelInterlockEXT',
        5379: 'DemoteToHelperInvocationEXT', 6025: 'GroupNonUniformRotateKHR', 28: 'UniformBufferArrayDynamicIndexing',
        29: 'SampledImageArrayDynamicIndexing', 30: 'StorageBufferArrayDynamicIndexing', 31: 'StorageImageArrayDynamicIndexing'}


def main():
    base, ids = sys.argv[1], sys.argv[2:]
    caps = collections.Counter()
    for gid in ids:
        d = os.path.join(base, gid)
        st, why, stages = collections.Counter(), collections.Counter(), collections.Counter()
        for ln in open(os.path.join(d, 'manifest.tsv'), encoding='utf-8'):
            if ln.startswith('#') or ln.startswith('key\t'):
                continue
            c = ln.rstrip('\n').split('\t')
            st[c[4]] += 1
            stages[c[1]] += 1
            if c[4] in ('failed', 'invalid', 'skipped'):
                j = json.load(open(os.path.join(d, 'spv', c[0] + '.json'), encoding='utf-8'))
                why[(c[4], re.sub(r'0x[0-9a-fA-F]+|\d+', 'N', j.get('why', ''))[:110])] += 1
            elif c[4] == 'ok':
                j = json.load(open(os.path.join(d, 'spv', c[0] + '.json'), encoding='utf-8'))
                for x in j.get('capabilities', []):
                    caps[x] += 1
        res = ''
        log = os.path.join(base, gid + '.log')
        if os.path.exists(log):
            res = next((l.strip() for l in open(log, encoding='utf-8', errors='replace') if l.startswith('RESULT found')), '')
        t = dict(re.findall(r'(\w+)=([\d.]+)', res))
        found = sum(st.values())
        print(f"{gid}: found {found} translated {found - st['failed'] - st['skipped']} failed {st['failed']} skipped {st['skipped']} "
              f"spirv-val clean {st['ok']} invalid {st['invalid']}; scan {t.get('scan_s')} s translate {t.get('translate_s')} s "
              f"validate {t.get('validate_s')} s total {t.get('total_s')} s; stages {dict(stages)}")
        for (s, w), n in why.most_common():
            print(f"    {n} {s}: {w}")
    print('capabilities over all valid outputs:')
    for k, n in sorted(caps.items()):
        print(f'    {k} {CAPS.get(k, "?")}: {n}')


if __name__ == '__main__':
    main()
