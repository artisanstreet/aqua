import base64, json, struct, zlib

# Cube: 8 corners, 12 triangles. CCW outward faces.
P = [(-1,-1,-1),(1,-1,-1),(1,1,-1),(-1,1,-1),
     (-1,-1,1),(1,-1,1),(1,1,1),(-1,-1+2,1)]
P = [(-1.,-1.,-1.),(1.,-1.,-1.),(1.,1.,-1.),(-1.,1.,-1.),
     (-1.,-1.,1.),(1.,-1.,1.),(1.,1.,1.),(-1.,1.,1.)]
IDX = [0,1,2, 0,2,3,  4,6,5, 4,7,6,  4,5,1, 4,1,0,
       3,2,6, 3,6,7,  1,5,6, 1,6,2,  4,0,3, 4,3,7]
pos = b''.join(struct.pack('<3f', *p) for p in P)
idx = b''.join(struct.pack('<H', i) for i in IDX)
blob = pos + idx
gltf = {
  "asset": {"version": "2.0", "generator": "aqua-fixture"},
  "scene": 0, "scenes": [{"nodes": [0]}],
  "nodes": [{"mesh": 0, "name": "Cube"}],
  "meshes": [{"name": "Cube",
              "primitives": [{"attributes": {"POSITION": 0}, "indices": 1}]}],
  "accessors": [
    {"bufferView": 0, "componentType": 5126, "count": 8,
     "type": "VEC3", "min": [-1., -1., -1.], "max": [1., 1., 1.]},
    {"bufferView": 1, "componentType": 5123, "count": 36, "type": "SCALAR"}],
  "bufferViews": [
    {"buffer": 0, "byteOffset": 0, "byteLength": len(pos),
     "target": 34962},
    {"buffer": 0, "byteOffset": len(pos), "byteLength": len(idx),
     "target": 34963}],
  "buffers": [{"byteLength": len(blob),
               "uri": "data:application/octet-stream;base64,"
                      + base64.b64encode(blob).decode()}],
}
open('crates/aqua/examples/assets/sample_cube.gltf', 'w').write(json.dumps(gltf, indent=2))
print("gltf bytes:", len(blob))
