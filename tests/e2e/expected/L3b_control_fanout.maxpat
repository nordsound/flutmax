{
  "patcher": {
    "fileversion": 1,
    "appversion": {
      "major": 8,
      "minor": 6,
      "revision": 0,
      "architecture": "x64",
      "modernui": 1
    },
    "classnamespace": "box",
    "rect": [100.0, 100.0, 640.0, 480.0],
    "bglocked": 0,
    "openinpresentation": 0,
    "default_fontsize": 12.0,
    "default_fontface": 0,
    "default_fontname": "Arial",
    "gridonopen": 1,
    "gridsize": [15.0, 15.0],
    "gridsnaponopen": 1,
    "objectsnaponopen": 1,
    "statusbarvisible": 2,
    "toolbarvisible": 1,
    "lefttoolbarpinned": 0,
    "toptoolbarpinned": 0,
    "righttoolbarpinned": 0,
    "bottomtoolbarpinned": 0,
    "toolbars_unpinned_last_save": 0,
    "tallnewobj": 0,
    "boxanimatetime": 200,
    "enablehscroll": 1,
    "enablevscroll": 1,
    "devicewidth": 0.0,
    "description": "",
    "digest": "",
    "tags": "",
    "style": "",
    "subpatcher_template": "",
    "assistshowspatchername": 0,
    "boxes": [
      {
        "box": {
          "id": "obj-1",
          "maxclass": "inlet",
          "numinlets": 0,
          "numoutlets": 1,
          "outlettype": [""],
          "patching_rect": [100.0, 50.0, 30.0, 30.0]
        }
      },
      {
        "box": {
          "id": "obj-2",
          "maxclass": "newobj",
          "text": "* 2",
          "numinlets": 2,
          "numoutlets": 1,
          "outlettype": [""],
          "patching_rect": [100.0, 120.0, 80.0, 22.0]
        }
      },
      {
        "box": {
          "id": "obj-3",
          "maxclass": "newobj",
          "text": "+ 100",
          "numinlets": 2,
          "numoutlets": 1,
          "outlettype": [""],
          "patching_rect": [100.0, 190.0, 80.0, 22.0]
        }
      },
      {
        "box": {
          "id": "obj-4",
          "maxclass": "newobj",
          "text": "- 100",
          "numinlets": 2,
          "numoutlets": 1,
          "outlettype": [""],
          "patching_rect": [200.0, 190.0, 80.0, 22.0]
        }
      },
      {
        "box": {
          "id": "obj-5",
          "maxclass": "newobj",
          "text": "trigger f f",
          "numinlets": 1,
          "numoutlets": 2,
          "outlettype": ["", ""],
          "patching_rect": [100.0, 260.0, 80.0, 22.0]
        }
      },
      {
        "box": {
          "id": "obj-6",
          "maxclass": "outlet",
          "numinlets": 1,
          "numoutlets": 0,
          "patching_rect": [100.0, 330.0, 30.0, 30.0]
        }
      },
      {
        "box": {
          "id": "obj-7",
          "maxclass": "outlet",
          "numinlets": 1,
          "numoutlets": 0,
          "patching_rect": [200.0, 330.0, 30.0, 30.0]
        }
      }
    ],
    "lines": [
      {
        "patchline": {
          "source": ["obj-1", 0],
          "destination": ["obj-2", 0]
        }
      },
      {
        "patchline": {
          "source": ["obj-2", 0],
          "destination": ["obj-5", 0]
        }
      },
      {
        "patchline": {
          "source": ["obj-5", 1],
          "destination": ["obj-3", 0]
        }
      },
      {
        "patchline": {
          "source": ["obj-5", 0],
          "destination": ["obj-4", 0]
        }
      },
      {
        "patchline": {
          "source": ["obj-3", 0],
          "destination": ["obj-6", 0]
        }
      },
      {
        "patchline": {
          "source": ["obj-4", 0],
          "destination": ["obj-7", 0]
        }
      }
    ],
    "dependency_cache": [],
    "autosave": 0
  }
}
