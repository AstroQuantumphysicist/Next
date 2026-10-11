import unittest
from accessibility_readback import parse

HEADER = b'PRISM A11Y BEGIN: frame=8 generation=2 context=4 token=0 count=2 focused=2\n'
ROOT = b'PRISM A11Y NODE: id=1 parent=0 role=Window enabled=1 visible=1 focused=0 selected=0 checked=-1 busy=0 bounds=40,50,420,280 name_hex=4e6578742053657474696e6773\n'
BUTTON = b'PRISM A11Y NODE: id=2 parent=1 role=Button enabled=1 visible=1 focused=1 selected=0 checked=-1 busy=0 bounds=50,60,44,32 name_hex=53617665\n'
END = b'PRISM A11Y END: frame=8 generation=2 count=2\n'
VALID = HEADER + ROOT + BUTTON + END

class Readback(unittest.TestCase):
    def test_actual_state_and_optional_geometry_are_preserved(self):
        state = parse(b'unrelated serial\n' + VALID)[0]
        self.assertEqual(state['nodes'][0]['name'], 'Next Settings')
        self.assertEqual(state['nodes'][1]['bounds'], (50,60,44,32))
        self.assertEqual(state['focused'], 2)
        no_geometry = VALID.replace(b'bounds=50,60,44,32', b'bounds=none')
        self.assertIsNone(parse(no_geometry)[0]['nodes'][1]['bounds'])
        self.assertEqual(len(parse(VALID + VALID)), 2)

    def test_corruption_never_becomes_complete_evidence(self):
        cases = (b'', HEADER + ROOT, END, VALID + HEADER, HEADER + HEADER,
                 VALID.replace(b'END: frame=8', b'END: frame=7'),
                 VALID.replace(b'count=2 focused', b'count=49 focused'),
                 VALID.replace(b'id=2 parent=1', b'id=1 parent=1'),
                 VALID.replace(b'id=2 parent=1', b'id=2 parent=9'),
                 VALID.replace(b'id=1 parent=0', b'id=1 parent=9'),
                 VALID.replace(b'focused=2\n', b'focused=1\n'),
                 VALID.replace(b'role=Button', b'role=Unknown'),
                 VALID.replace(b'name_hex=53617665', b'name_hex=ff'),
                 VALID.replace(b'name_hex=53617665', b'name_hex=00'),
                 VALID.replace(b'name_hex=53617665', b'name_hex=1'),
                 VALID.replace(b'name_hex=53617665', b'name_hex=' + b'61'*129),
                 VALID.replace(b'bounds=50,60,44,32', b'bounds=50,60,-1,32'),
                 VALID.replace(b'bounds=50,60,44,32', b'bounds=2147483648,60,44,32'),
                 VALID.replace(b'role=Button enabled=1', b'role=Button enabled=0'),
                 VALID.replace(b'role=Window enabled=1 visible=1 focused=0', b'role=Window enabled=1 visible=1 focused=1'),
                 VALID + VALID.replace(b'frame=8', b'frame=7'),
                 VALID + VALID.replace(b'generation=2', b'generation=1'))
        for value in cases:
            with self.subTest(value=value), self.assertRaises(ValueError):
                parse(value)

if __name__ == '__main__':
    unittest.main()
